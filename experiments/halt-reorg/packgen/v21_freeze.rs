// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Paranoid Zero.

//! Freeze the early-finance (v2@10 / v2.1@24) twin of the release v2.1
//! three-class finance pack. Class shapes are pinned to the release pack
//! (Small 63/504/63 m23, Large 206/504/63 m24, Native 63/504/63 m24 with
//! headers 5, actions 128, markets 4, policies 48, funding 64, settlement
//! 34); only the schedule and authorization activation come from this
//! executable. Matrices are frozen through the same
//! `capacity_banked::assemble_frozen` path the live node verifies with.
//!
//! Usage (noid_v2_capacity joint-freeze-v21-early):
//!   LEGACY_PACK LEGACY_PIN RELEASE_V21_METADATA RELEASE_V21_BANK NEW_OUTPUT
//!
//! The release v2.1 metadata is used only to seed canonical placeholder
//! block VKs for the parts bootstrap; the derived VKs and every frozen
//! matrix depend only on this executable's config.

use super::*;
use noid_chain::consensus::params;
use noid_miner::{history_step_artifacts as old_pack, v21_artifacts as new_pack};
use noid_recursive::acceptance::history_step::v2::capacity_banked as capacity;
use noid_recursive::acceptance::trace::native_assets::sequential_trade::kernel::TradeKernelConfig;
use noid_recursive::HistoryStepMatrixLease;

struct RejectSource;
impl capacity::MatrixSource for RejectSource {
    fn load(&self, _: capacity::Class) -> std::result::Result<HistoryStepMatrixLease, v2::V2Error> {
        Err(v2::V2Error::Matrix)
    }
}

pub fn freeze_v21_early(args: &[String]) -> Result<()> {
    if args.len() != 5 {
        return Err(
            "usage: noid_v2_capacity joint-freeze-v21-early LEGACY_PACK LEGACY_PIN \
RELEASE_V21_METADATA RELEASE_V21_BANK NEW_OUTPUT"
                .into(),
        );
    }
    let schedule = noid_chain::consensus::forks::ACTIVE_SCHEDULE;
    if params::ISOLATED_V1_1_TESTNET
        || params::ISOLATED_V2_FORK_TESTNET
        || schedule.v2().map(|a| a.height()) != Some(params::MAINNET_V2_ACTIVATION_HEIGHT)
    {
        return Err("early-finance v2.1 freezing requires the source-pinned early profile".into());
    }
    let authorization = params::OWNER_AUTHORIZATION_ACTIVATION_HEIGHT
        .ok_or("v2.1 authorization activation must be set")?;
    if authorization <= params::MAINNET_V2_ACTIVATION_HEIGHT {
        return Err("v2.1 activation must follow the v2 activation".into());
    }

    // Release-pinned class shapes (see noid_pack_probe on the release pack).
    let small = v2::V2Config::with_limits(23, 63, 504, 63, schedule)
        .map_err(err)?
        .with_authorization_update(authorization)
        .map_err(err)?
        .with_native_assets(0)
        .map_err(err)?;
    let large = v2::V2Config::with_limits(24, 206, 504, 63, schedule)
        .map_err(err)?
        .with_authorization_update(authorization)
        .map_err(err)?
        .with_native_assets(0)
        .map_err(err)?;
    let native = v2::V2Config::with_limits(24, 63, 504, 63, schedule)
        .map_err(err)?
        .with_authorization_update(authorization)
        .map_err(err)?
        .with_native_sequential_finance(
            5,
            128,
            TradeKernelConfig {
                markets: 4,
                policies: 48,
                funding: 64,
            },
        )
        .map_err(err)?;
    let config = capacity::Config::new(small, large, native)
        .map_err(err)?
        // Release-pinned structural flags (see noid_pack_probe on the
        // release pack): without shared_m24 + reused linear rows the Large
        // class exceeds its 2^24 wire budget by ~619k wires.
        .with_fused_pins()
        .with_reused_linear_rows()
        .with_shared_m24();

    let legacy_pin = digest(&args[1])?;
    let legacy_path = Path::new(&args[0])
        .join(old_pack::HISTORY_STEP_PACK_VERSION_DIRECTORY)
        .join(old_pack::HISTORY_STEP_RUNTIME_METADATA_FILE);
    let legacy = old_pack::decode_history_step_runtime_metadata_pinned(
        &proof::bounded(
            &legacy_path,
            old_pack::HISTORY_STEP_RUNTIME_METADATA_MAX_BYTES,
        )?,
        legacy_pin,
    )
    .map_err(err)?;

    let release_bank = digest(&args[3])?;
    let release = new_pack::decode_v21_runtime_metadata_pinned(
        &proof::bounded(Path::new(&args[2]), new_pack::V21_METADATA_MAX_BYTES)?,
        release_bank,
    )
    .map_err(err)?;

    let output = Path::new(&args[4]);
    std::fs::create_dir(output).map_err(|e| format!("new output directory required: {e}"))?;
    let started = Instant::now();

    let (chain, epoch) = Chain::for_matrix_freezing(config.class(capacity::Class::Small), 0)?;
    let ghost = noid_recursive::prepare_history_step_ghost_authorization_for_version(
        noid_gkr::ghost_tx::prove_selected_ghost_authorization_for_version(
            config.class(capacity::Class::Small).authorization_version(),
        )
        .map_err(err)?,
        config.class(capacity::Class::Small).authorization_version(),
    )
    .map_err(err)?;
    let (first, _) = chain.build(&[], config.schedule())?;

    // Bootstrap: canonical placeholder VKs from the release parts (same
    // geometry; only schedule/activation differ), layouts fixed under this
    // executable's config, then the real VKs witness-only.
    let placeholder_vks = capacity::Class::ALL.map(|class| release.parts().block_vk(class).clone());
    let parts0 = capacity::derive_runtime_parts(config, placeholder_vks).map_err(err)?;
    let runtime0 = capacity::Runtime::new(
        capacity::Bank::pin([[0; 32]; 3], &parts0),
        parts0,
        Box::new(RejectSource),
    )
    .map_err(err)?;
    let origin0 = capacity::Origin::for_matrix_freezing(
        legacy.bank().digest(),
        runtime0.bank(),
        chain.parent(),
        &epoch,
        &chain.accumulator,
    )
    .map_err(err)?;

    let vk_small = {
        let input =
            chain.input::<63>(&first, &Batch::default(), &ghost, config.class(capacity::Class::Small))?;
        capacity::derive_assembled_block_vk(&runtime0, capacity::Class::Small, &origin0, None, input)
            .map_err(|e| format!("vk derivation Small: {e:?}"))?
            .0
    };
    let vk_large = {
        let input =
            chain.input::<206>(&first, &Batch::default(), &ghost, config.class(capacity::Class::Large))?;
        capacity::derive_assembled_block_vk(&runtime0, capacity::Class::Large, &origin0, None, input)
            .map_err(|e| format!("vk derivation Large: {e:?}"))?
            .0
    };
    let vk_native = {
        let input =
            chain.input::<63>(&first, &Batch::default(), &ghost, config.class(capacity::Class::Native))?;
        capacity::derive_assembled_block_vk(&runtime0, capacity::Class::Native, &origin0, None, input)
            .map_err(|e| format!("vk derivation Native: {e:?}"))?
            .0
    };
    let mut parts =
        capacity::derive_runtime_parts(config, [vk_small, vk_large, vk_native]).map_err(err)?;

    // Freeze all three classes under the provisional pins until the frozen
    // block VKs match the parts (witness-only VK derivation is pin-free).
    let mut converged = None;
    for pass in 0..6 {
        let bank = capacity::Bank::pin([[0; 32]; 3], &parts);
        let runtime =
            capacity::Runtime::new(bank, parts.clone(), Box::new(RejectSource)).map_err(err)?;
        let origin = capacity::Origin::for_matrix_freezing(
            legacy.bank().digest(),
            runtime.bank(),
            chain.parent(),
            &epoch,
            &chain.accumulator,
        )
        .map_err(err)?;
        let mut vks = Vec::new();
        let mut digests = Vec::new();
        let mut rows = [0usize; 3];
        let mut paths = Vec::new();
        for (class, pages) in [
            (capacity::Class::Small, 63usize),
            (capacity::Class::Large, 206usize),
            (capacity::Class::Native, 63usize),
        ] {
            let phase = Instant::now();
            let frozen =
                freeze_class(&runtime, &origin, &chain, &first, &ghost, class, pages)?;
            if frozen.parent_vk() != runtime.parts().parent_vk() {
                return Err("v2.1 joint parent geometry changed".into());
            }
            let path = output.join(format!("pass{pass}-class{}.field-r1cs.zst", class.wire_id()));
            proof::write_matrix(&path, frozen.matrix())?;
            println!(
                "{}",
                json!({"phase":"v21_early_freeze","pass":pass,"class":class.wire_id(),
                "rows":frozen.matrix().useful_rows,"satisfied":true,
                "matrix":hex::encode(frozen.matrix().statement_digest()),
                "ms":elapsed(phase),"memory":memory()})
            );
            vks.push(frozen.block_vk().clone());
            digests.push(frozen.matrix().statement_digest());
            rows[class.index()] = frozen.matrix().useful_rows;
            paths.push(path);
        }
        let vks = [vks[0].clone(), vks[1].clone(), vks[2].clone()];
        let digests = [digests[0], digests[1], digests[2]];
        if capacity::Class::ALL
            .into_iter()
            .all(|class| &vks[class.index()] == parts.block_vk(class))
        {
            converged = Some((digests, rows, paths));
            break;
        }
        parts = capacity::derive_runtime_parts(config, vks).map_err(err)?;
    }
    let (digests, rows, paths) = converged.ok_or("v2.1 joint geometry did not converge")?;

    let bank = capacity::Bank::pin(digests, &parts);
    let runtime =
        capacity::Runtime::new(bank, parts.clone(), Box::new(RejectSource)).map_err(err)?;

    // Witness invariance spot check: re-freeze Small under a different
    // hypothetical boundary; its matrix digest and block VK must not move.
    let (chain_b, epoch_b) = Chain::for_matrix_freezing(config.class(capacity::Class::Small), 1)?;
    let (block_b, _) = chain_b.build(&[], config.schedule())?;
    let origin_b = capacity::Origin::for_matrix_freezing(
        legacy.bank().digest(),
        runtime.bank(),
        chain_b.parent(),
        &epoch_b,
        &chain_b.accumulator,
    )
    .map_err(err)?;
    let frozen_b = freeze_class(
        &runtime,
        &origin_b,
        &chain_b,
        &block_b,
        &ghost,
        capacity::Class::Small,
        63,
    )?;
    if frozen_b.matrix().statement_digest() != digests[capacity::Class::Small.index()]
        || frozen_b.block_vk() != parts.block_vk(capacity::Class::Small)
    {
        return Err("v2.1 matrix changed with the boundary witness".into());
    }
    println!(
        "{}",
        json!({"phase":"v21_witness_invariance","class":0,"satisfied":true,
        "scope":"Small only","memory":memory()})
    );

    let encoded = new_pack::encode_v21_runtime_metadata(runtime.bank(), &parts).map_err(err)?;
    let metadata =
        new_pack::decode_v21_runtime_metadata_pinned(&encoded, runtime.bank().digest())
            .map_err(err)?;
    let mut entries = Vec::new();
    for (class, path) in capacity::Class::ALL.iter().zip(paths.iter()) {
        let compressed = proof::bounded(path, new_pack::V21_MATRIX_MAX_BYTES)?;
        metadata.preflight_build_matrix(*class, &compressed)?;
        let name = new_pack::v21_matrix_file_name(*class);
        std::fs::write(output.join(name), &compressed).map_err(|e| e.to_string())?;
        let limits = config.class(*class);
        entries.push(json!({"class":class.wire_id(),"m":limits.outer_m(),
            "pages":limits.pages(),"inputs":limits.max_live_inputs(),
            "calls":limits.contract_slots(),"rows":rows[class.index()],
            "matrix":hex::encode(digests[class.index()]),
            "matrix_zstd_bytes":compressed.len()}));
    }
    std::fs::write(output.join(new_pack::V21_METADATA_FILE), encoded).map_err(|e| e.to_string())?;
    let report = json!({
        "status":"early-finance v2.1 matrix pack; no verified origin",
        "bank":hex::encode(runtime.bank().digest()),
        "entries":entries,
        "legacy_metadata":hex::encode(legacy_pin),
        "schedule":{"v1_1_height":schedule.v1_1_height(),
            "v2_height":config.activation_height(),
            "block_seconds":schedule.block_time(config.activation_height()),
            "authorization_activation":authorization},
        "witness_invariance":"Small class only",
        "verified_origin_created":false,
        "elapsed_ms":elapsed(started),"memory":memory(),
        "rayon_threads":rayon::current_num_threads(),
        "cpu_backend":noid_core::cpu::selected_backend().to_string(),
    });
    std::fs::write(
        output.join("v21.pins.json"),
        serde_json::to_vec_pretty(&report).map_err(err)?,
    )
    .map_err(|e| e.to_string())?;
    println!("{}", serde_json::to_string_pretty(&report).map_err(err)?);
    Ok(())
}

fn freeze_class(
    runtime: &capacity::Runtime,
    origin: &capacity::Origin,
    chain: &Chain,
    block: &Block,
    ghost: &PreparedHistoryStepGhostAuthorization,
    class: capacity::Class,
    pages: usize,
) -> Result<v2::FrozenV2> {
    let config = runtime.bank().config().class(class);
    let frozen = match pages {
        63 => {
            let input = chain.input::<63>(block, &Batch::default(), ghost, config)?;
            capacity::assemble_frozen(runtime, class, origin, None, input).map_err(err)?
        }
        206 => {
            let input = chain.input::<206>(block, &Batch::default(), ghost, config)?;
            capacity::assemble_frozen(runtime, class, origin, None, input).map_err(err)?
        }
        other => return Err(format!("unsupported page count {other}")),
    };
    if !frozen.matrix().satisfies(frozen.witness()) {
        return Err("v2.1 joint matrix witness unsatisfied".into());
    }
    Ok(frozen)
}
