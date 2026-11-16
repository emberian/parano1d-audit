//! Release-schedule HistoryStep runtime loading and block production for the
//! split-cd1 alias witness. Adapted from `experiments/verification-wallet/
//! src/full_history.rs` (proven working template) with paths anchored to this
//! lane's directory.

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context, Result};
use noid_chain::consensus::pow::search_pow;
use noid_chain::storage::MdbxChainContext;
use noid_chain::{AcceptedBlockBundle, Block};
use noid_ivc_core::field_r1cs::CompactFieldR1cs;
use noid_mempool::{AsyncMempool, ChainView, MempoolConfig};
use noid_miner::history_step_artifacts::{
    decode_history_step_runtime_metadata_pinned, history_step_matrix_file_name,
    HISTORY_STEP_PACK_VERSION_DIRECTORY, HISTORY_STEP_RUNTIME_METADATA_FILE,
    HISTORY_STEP_RUNTIME_METADATA_MAX_BYTES,
};
use noid_miner::template::{TemplateBuilder, TemplateChainSnapshot};
use noid_miner::v21_artifacts::{
    decode_v21_runtime_metadata_pinned, v21_matrix_file_name, V21_MATRIX_MAX_BYTES,
    V21_METADATA_FILE, V21_METADATA_MAX_BYTES,
};
use noid_miner::v2_artifacts::{
    decode_v2_runtime_metadata_pinned, v2_matrix_file_name, V2_MATRIX_MAX_BYTES, V2_METADATA_FILE,
    V2_METADATA_MAX_BYTES,
};
use noid_miner::{HistoryProtocolRuntime, PreparedBlockAttempt};
use noid_poseidon2b::primitives::Address;
use noid_recursive::acceptance::history_step::v2::{banked as v2, capacity_banked as v21};
use noid_recursive::{
    canonical_history_step_shape, CanonicalHistoryStepClassId, HistoryStepMatrixLease,
    HistoryStepMatrixSource, HistoryStepMatrixSourceError, HistoryStepRuntime,
    PreparedHistoryStepGhostAuthorization,
};

pub const PINNED_COMMIT: &str = "52938433a30b185ab007afaa15166a632cf2e82e";

pub const LEGACY_METADATA_PIN_HEX: &str =
    "ad463bd76e27df3c0f414f4fd7640cfb5c45cc7f3a09e44a2f7bc7a8b869485b";
pub const V2_BANK_PIN_HEX: &str =
    "c2a6df736b0d0da22e285b6930b11cf44b520d65b52c7dfe78f44fe0cd48e76e";
pub const V21_BANK_PIN_HEX: &str =
    "6c280ae869afc25e31d46d73b42870512991b78c9498bc7e5ef939bc07e5ed5e";
pub const RETIREMENT_KEY_0_PIN_HEX: &str =
    "0651ce507810b61213cbdc0e9f99436aec3a7922cc662bfba6128b466453c8b2";
pub const RETIREMENT_KEY_1_PIN_HEX: &str =
    "c301274d50dc02fd5c383f2e71629075729a954a768f5051388ec2040bce0267";

const MAX_CANONICAL_MATRIX_BYTES: usize = 1024 * 1024 * 1024;

pub fn lane_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("lane dir")
        .to_path_buf()
}

pub fn experiments_dir() -> PathBuf {
    lane_dir()
        .parent()
        .expect("experiments dir")
        .to_path_buf()
}

/// Pack root: `NOID_SPLIT_CD1_PACKS_ROOT` when set (hbox layout:
/// `<root>/{history-step-v1, downloads/v2-pack/..., downloads/v21-pack/...}`),
/// otherwise the Mac devnet-chaos layout.
fn packs_root() -> PathBuf {
    std::env::var("NOID_SPLIT_CD1_PACKS_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| experiments_dir().join("devnet-chaos/packs"))
}

pub fn legacy_pack_dir() -> PathBuf {
    packs_root().join("history-step-v1")
}

pub fn v2_pack_dir() -> PathBuf {
    packs_root().join("downloads/v2-pack/history-step-pack-v2")
}

pub fn v21_pack_dir() -> PathBuf {
    packs_root().join("downloads/v21-pack/history-step-pack-v21")
}

pub fn parse_pin_hex(s: &str) -> Result<[u8; 32]> {
    let raw = hex::decode(s).context("decode hex pin")?;
    raw.try_into()
        .map_err(|_| anyhow::anyhow!("expected 32-byte digest"))
}

pub fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>> {
    let meta = fs::symlink_metadata(path)
        .with_context(|| format!("stat bounded file {}", path.display()))?;
    if !meta.is_file() || meta.len() > limit as u64 {
        bail!(
            "file {} exceeds bound {} (actual {})",
            path.display(),
            limit,
            meta.len()
        );
    }
    let mut out = Vec::new();
    File::open(path)
        .with_context(|| format!("open {}", path.display()))?
        .take(limit as u64 + 1)
        .read_to_end(&mut out)
        .with_context(|| format!("read {}", path.display()))?;
    if out.len() > limit {
        bail!("file {} grew past bound {}", path.display(), limit);
    }
    Ok(out)
}

struct FileLegacyMatrixSource {
    v1_dir: PathBuf,
    digests: [[u8; 32]; 2],
    cache: Mutex<[Option<Arc<CompactFieldR1cs>>; 2]>,
}

impl HistoryStepMatrixSource for FileLegacyMatrixSource {
    fn load(
        &self,
        class: CanonicalHistoryStepClassId,
    ) -> std::result::Result<HistoryStepMatrixLease, HistoryStepMatrixSourceError> {
        let mut guard = self.cache.lock().map_err(|_| HistoryStepMatrixSourceError)?;
        let idx = class.index();
        if let Some(existing) = &guard[idx] {
            return Ok(HistoryStepMatrixLease::Compact(Arc::clone(existing)));
        }
        let path = self.v1_dir.join(history_step_matrix_file_name(class));
        let compressed =
            read_bounded(&path, MAX_CANONICAL_MATRIX_BYTES).map_err(|_| HistoryStepMatrixSourceError)?;
        let mut decoder = zstd::stream::read::Decoder::new(compressed.as_slice())
            .map_err(|_| HistoryStepMatrixSourceError)?;
        decoder
            .window_log_max(27)
            .map_err(|_| HistoryStepMatrixSourceError)?;
        let mut canonical = Vec::new();
        decoder
            .take(MAX_CANONICAL_MATRIX_BYTES as u64 + 1)
            .read_to_end(&mut canonical)
            .map_err(|_| HistoryStepMatrixSourceError)?;
        if canonical.len() > MAX_CANONICAL_MATRIX_BYTES {
            return Err(HistoryStepMatrixSourceError);
        }
        let shape = canonical_history_step_shape(class);
        let compact = CompactFieldR1cs::open(canonical.into_boxed_slice(), shape, self.digests[idx])
            .and_then(CompactFieldR1cs::into_startup_packed)
            .map_err(|_| HistoryStepMatrixSourceError)?;
        let arc = Arc::new(compact);
        guard[idx] = Some(Arc::clone(&arc));
        Ok(HistoryStepMatrixLease::Compact(arc))
    }
}

struct SharedLegacyMatrixSource(Arc<FileLegacyMatrixSource>);

impl HistoryStepMatrixSource for SharedLegacyMatrixSource {
    fn load(
        &self,
        class: CanonicalHistoryStepClassId,
    ) -> std::result::Result<HistoryStepMatrixLease, HistoryStepMatrixSourceError> {
        self.0.load(class)
    }
}

/// Load a complete release-schedule `HistoryProtocolRuntime` (legacy + v2 + v2.1
/// + retirement keys) using full semantic `CompactFieldR1cs::open` verification.
pub fn load_full_history_protocol_runtime(origin_dir: PathBuf) -> Result<Arc<HistoryProtocolRuntime>> {
    assert!(
        !cfg!(debug_assertions),
        "run with `--release` so FRI/capsule query parameters match release (64/64/16)"
    );
    let legacy_v1 = legacy_pack_dir().join(HISTORY_STEP_PACK_VERSION_DIRECTORY);
    let legacy_meta_bytes = read_bounded(
        &legacy_v1.join(HISTORY_STEP_RUNTIME_METADATA_FILE),
        HISTORY_STEP_RUNTIME_METADATA_MAX_BYTES,
    )?;
    let legacy_meta = decode_history_step_runtime_metadata_pinned(
        &legacy_meta_bytes,
        parse_pin_hex(LEGACY_METADATA_PIN_HEX)?,
    )?;
    let legacy_digests = [
        legacy_meta.bank().entries()[0].matrix_digest(),
        legacy_meta.bank().entries()[1].matrix_digest(),
    ];
    let (legacy_bank, legacy_parts) = legacy_meta.into_parts();
    let legacy_source = Arc::new(FileLegacyMatrixSource {
        v1_dir: legacy_v1,
        digests: legacy_digests,
        cache: Mutex::new([None, None]),
    });

    let v2_dir = v2_pack_dir();
    let v2_meta = decode_v2_runtime_metadata_pinned(
        &read_bounded(&v2_dir.join(V2_METADATA_FILE), V2_METADATA_MAX_BYTES)?,
        parse_pin_hex(V2_BANK_PIN_HEX)?,
    )
    .map_err(anyhow::Error::msg)?;
    let v2_compressed = v2::Class::ALL.map(|class| {
        Arc::<[u8]>::from(
            read_bounded(&v2_dir.join(v2_matrix_file_name(class)), V2_MATRIX_MAX_BYTES)
                .expect("read v2 matrix"),
        )
    });
    let v2_runtime = Arc::new(
        v2_meta
            .into_runtime(v2_compressed)
            .map_err(anyhow::Error::msg)?,
    );

    let v21_dir = v21_pack_dir();
    let v21_meta = decode_v21_runtime_metadata_pinned(
        &read_bounded(&v21_dir.join(V21_METADATA_FILE), V21_METADATA_MAX_BYTES)?,
        parse_pin_hex(V21_BANK_PIN_HEX)?,
    )
    .map_err(anyhow::Error::msg)?;
    let v21_compressed = v21::Class::ALL.map(|class| {
        Arc::<[u8]>::from(
            read_bounded(
                &v21_dir.join(v21_matrix_file_name(class)),
                V21_MATRIX_MAX_BYTES,
            )
                .expect("read v21 matrix"),
        )
    });
    let v21_runtime = Arc::new(
        v21_meta
            .into_runtime(v21_compressed)
            .map_err(anyhow::Error::msg)?,
    );

    let key0 = read_bounded(&v2_dir.join("retirement-keys/class-0.key"), 512)?;
    let key1 = read_bounded(&v2_dir.join("retirement-keys/class-1.key"), 512)?;
    let retirement_keys = v2::PinnedRetirementKeys::from_release(
        &legacy_bank,
        [&key0, &key1],
        [
            parse_pin_hex(RETIREMENT_KEY_0_PIN_HEX)?,
            parse_pin_hex(RETIREMENT_KEY_1_PIN_HEX)?,
        ],
    )
    .map_err(|e| anyhow::anyhow!("retirement keys: {e}"))?;

    let legacy_runtime = Arc::new(HistoryStepRuntime::new(
        legacy_bank,
        Box::new(SharedLegacyMatrixSource(legacy_source)),
        legacy_parts,
    )?);

    let runtime = HistoryProtocolRuntime::new(
        Some(legacy_runtime),
        Some(v2_runtime),
        origin_dir,
    )
    .map_err(anyhow::Error::msg)?
    .with_finance_runtime(Some(v21_runtime))
    .map_err(anyhow::Error::msg)?
    .with_retirement_keys(retirement_keys)
    .map_err(anyhow::Error::msg)?;

    Ok(Arc::new(runtime))
}

pub fn prepare_v3_ghost_auth() -> Result<Arc<PreparedHistoryStepGhostAuthorization>> {
    let _ = noid_miner::configure_process_cpu_budget(noid_miner::ProcessCpuBudgetMode::InternalMiner);
    noid_miner::install_history_step_phase_cpu(|| {
        let proof = noid_gkr::ghost_tx::prove_selected_ghost_authorization_for_version(
            noid_tx::OwnerAuthorizationVersion::V3,
        )
        .map_err(|e| anyhow::anyhow!("ghost auth prove: {e}"))?;
        noid_recursive::acceptance::history_step::prepare_history_step_ghost_authorization_for_version(
            proof,
            noid_tx::OwnerAuthorizationVersion::V3,
        )
        .map(Arc::new)
        .map_err(|e| anyhow::anyhow!("ghost auth prepare: {e}"))
    })
    .map_err(|e| anyhow::anyhow!("ghost auth cpu phase: {e}"))?
}

/// Produce and commit one genuine HistoryStep-proved coinbase block onto `ctx`.
pub async fn produce_and_commit_block(
    ctx: &mut MdbxChainContext,
    runtime: &HistoryProtocolRuntime,
    ghost: &PreparedHistoryStepGhostAuthorization,
    miner_address: Address,
    desired_timestamp: u64,
    nonce_seed: u128,
) -> Result<(Block, AcceptedBlockBundle)> {
    let mempool = AsyncMempool::new(ChainView::from_mdbx(ctx), MempoolConfig::default());
    let builder = TemplateBuilder::new(mempool).with_history_protocol(runtime);
    let snapshot = TemplateChainSnapshot::from_context(ctx).context("TemplateChainSnapshot")?;
    let template = builder
        .build_from_snapshot(snapshot, miner_address, desired_timestamp)
        .await
        .ok_or_else(|| anyhow::anyhow!("build_from_snapshot returned None"))?;
    let block_ts = template.timestamp;
    let attempt = noid_miner::install_history_step_phase_cpu(|| {
        PreparedBlockAttempt::prepare(template, runtime, ghost, block_ts)
    })
    .map_err(|e| anyhow::anyhow!("history_step cpu phase: {e}"))?
    .map_err(|e| anyhow::anyhow!("PreparedBlockAttempt::prepare: {e}"))?;

    let pow_hdr = attempt.pow_header(0);
    let mut cursor = nonce_seed;
    let winning_nonce = loop {
        if let Some(found) = search_pow(&pow_hdr, cursor, 131_072) {
            break found;
        }
        cursor = cursor.wrapping_add(131_072);
    };
    let proved = attempt
        .prove(runtime, winning_nonce)
        .map_err(|e| anyhow::anyhow!("PreparedBlockAttempt::prove: {e}"))?;
    let committed = proved.commit(ctx).context("ProvedBlock::commit")?;
    Ok(committed.into_parts())
}
