//! Shared conformance-test helpers for `parano1d` synchronization, ASERT/MTP,
//! mining readiness, and hard-finality boundary verification.
//!
//! All helpers operate strictly on local fixtures or `127.0.0.1` loopback.

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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

pub fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

pub fn release_daemon_bin() -> PathBuf {
    workspace_root().join("experiments/devnet-chaos/bin/parano1d")
}

pub fn release_cli_bin() -> PathBuf {
    workspace_root().join("experiments/devnet-chaos/bin/parano1d-cli")
}

pub fn legacy_pack_dir() -> PathBuf {
    workspace_root().join("experiments/devnet-chaos/packs/history-step-v1")
}

pub fn v2_pack_dir() -> PathBuf {
    workspace_root().join("experiments/devnet-chaos/downloads/v2-pack/history-step-pack-v2")
}

pub fn v21_pack_dir() -> PathBuf {
    workspace_root().join("experiments/devnet-chaos/downloads/v21-pack/history-step-pack-v21")
}

pub fn mainnet_retirement_origin_path() -> PathBuf {
    workspace_root().join("parano1d/noid_node/assets/mainnet-retirement.origin")
}

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("monotonic wall clock")
        .as_secs()
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
        "run with `--profile fast` or `--release` so FRI/capsule query parameters match release (64/64/16)"
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

/// Strip ANSI escape sequences emitted by tracing subscribers.
pub fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// Loopback-only, PID-scoped `parano1d` daemon wrapper.
pub struct LocalDaemon {
    pub name: String,
    pub data_dir: PathBuf,
    pub config_path: PathBuf,
    pub log_path: PathBuf,
    pub pid_path: PathBuf,
    pub p2p_port: u16,
    pub rpc_port: u16,
    child: Option<Child>,
}

impl LocalDaemon {
    pub fn spawn(
        run_root: &Path,
        name: &str,
        data_dir: PathBuf,
        p2p_port: u16,
        rpc_port: u16,
        extra_args: &[&str],
    ) -> Result<Self> {
        fs::create_dir_all(run_root)?;
        fs::create_dir_all(&data_dir)?;
        let config_path = run_root.join(format!("{name}.toml"));
        fs::write(&config_path, "")?;
        let log_path = run_root.join(format!("{name}.log"));
        let pid_path = run_root.join(format!("{name}.pid"));
        let log_file = File::create(&log_path)?;
        let log_err = log_file.try_clone()?;

        let mut cmd = Command::new(release_daemon_bin());
        cmd.arg("--config")
            .arg(&config_path)
            .arg("--data-dir")
            .arg(&data_dir)
            .arg("--p2p-listen")
            .arg(format!("127.0.0.1:{p2p_port}"))
            .arg("--rpc-listen")
            .arg(format!("127.0.0.1:{rpc_port}"))
            .arg("--disable-dns-seeds")
            .arg("--log")
            .arg("debug");
        for arg in extra_args {
            cmd.arg(arg);
        }
        cmd.stdout(Stdio::from(log_file))
            .stderr(Stdio::from(log_err));

        let child = cmd
            .spawn()
            .with_context(|| format!("spawn parano1d daemon {name}"))?;
        fs::write(&pid_path, child.id().to_string())?;
        Ok(Self {
            name: name.to_string(),
            data_dir,
            config_path,
            log_path,
            pid_path,
            p2p_port,
            rpc_port,
            child: Some(child),
        })
    }

    pub fn rpc_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.rpc_port)
    }

    pub fn p2p_addr(&self) -> String {
        format!("127.0.0.1:{}", self.p2p_port)
    }

    pub async fn rpc(
        &self,
        method: &str,
        params: serde_json::Value,
        bearer: Option<&str>,
    ) -> Result<serde_json::Value> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()?;
        let mut req = client.post(self.rpc_url()).json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": format!("paranoid_{method}"),
            "params": params,
        }));
        if let Some(token) = bearer {
            req = req.bearer_auth(token);
        }
        let resp: serde_json::Value = req.send().await?.json().await?;
        if let Some(err) = resp.get("error") {
            bail!("RPC {method} error: {err}");
        }
        Ok(resp.get("result").cloned().unwrap_or(serde_json::Value::Null))
    }

    pub async fn wait_rpc_ready(&self, timeout: Duration) -> Result<serde_json::Value> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Ok(info) = self.rpc("getChainInfo", serde_json::json!([]), None).await {
                if info.get("height").is_some() {
                    return Ok(info);
                }
            }
            if Instant::now() >= deadline {
                bail!(
                    "daemon {} RPC did not become ready within {:?}; tail:\n{}",
                    self.name,
                    timeout,
                    self.log_tail(25)
                );
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }

    pub async fn chain_tip(&self) -> Result<(u64, String)> {
        let info = self.rpc("getChainInfo", serde_json::json!([]), None).await?;
        let h = info["height"]
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("missing height in {info}"))?;
        let hash = info["best_hash"]
            .as_str()
            .unwrap_or("")
            .to_string();
        Ok((h, hash))
    }

    pub fn read_clean_log(&self) -> String {
        fs::read_to_string(&self.log_path)
            .map(|s| strip_ansi(&s))
            .unwrap_or_default()
    }

    pub fn log_tail(&self, n: usize) -> String {
        let text = self.read_clean_log();
        let lines: Vec<&str> = text.lines().collect();
        let start = lines.len().saturating_sub(n);
        lines[start..].join("\n")
    }

    pub fn shutdown(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        let pid = child.id() as i32;
        if child.try_wait().ok().flatten().is_none() {
            unsafe {
                libc::kill(pid, libc::SIGTERM);
            }
            let deadline = Instant::now() + Duration::from_secs(6);
            while Instant::now() < deadline {
                if child.try_wait().ok().flatten().is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        let _ = fs::remove_file(&self.pid_path);
    }
}

impl Drop for LocalDaemon {
    fn drop(&mut self) {
        self.shutdown();
    }
}
