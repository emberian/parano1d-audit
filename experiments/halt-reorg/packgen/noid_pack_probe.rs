// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2026 Paranoid Zero.

//! Dump the schedule and per-class parameters of a release v2 or v2.1 pack's
//! runtime metadata, so early-schedule twins can be generated with identical
//! shapes.
//!
//! Usage:
//!   noid_pack_probe v2  <metadata.bin> <release-bank-hex>
//!   noid_pack_probe v21 <metadata.bin> <release-bank-hex>

use std::fs;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        eprintln!(
            "usage: noid_pack_probe v2|v21 <metadata.bin> <release-bank-hex>"
        );
        std::process::exit(2);
    }
    let metadata_path = &args[1];
    let bank_hex = args[2].trim().to_string();
    let bank: [u8; 32] = hex::decode(&bank_hex)
        .expect("bank hex")
        .try_into()
        .expect("bank must be 32 bytes");
    let bytes = fs::read(metadata_path).expect("read metadata");
    match args[0].as_str() {
        "v2" => {
            let metadata =
                noid_miner::v2_artifacts::decode_v2_runtime_metadata_pinned(&bytes, bank)
                    .expect("decode v2 metadata");
            let config = metadata.bank().config();
            println!("v2 pack: {}", metadata_path);
            println!(
                "  schedule: v1_1={:?} v2={:?}",
                config.schedule().v1_1_height(),
                config
                    .schedule()
                    .v2()
                    .map(|a| (a.height(), a.block_time()))
            );
            for class in noid_recursive::acceptance::history_step::v2::banked::Class::ALL {
                let c = config.class(class);
                println!(
                    "  class wire={} m={} pages={} inputs={} calls={} auth_activation={:?} \
native_headers={:?} native_financial={:?} native_sequential={:?}",
                    class.wire_id(),
                    c.outer_m(),
                    c.pages(),
                    c.max_live_inputs(),
                    c.contract_slots(),
                    c.authorization_activation(),
                    c.native_header_capacity(),
                    c.native_financial_config().is_some(),
                    c.native_sequential_config().is_some(),
                );
                println!(
                    "    matrix_digest={}",
                    hex::encode(metadata.bank().matrix_digest(class))
                );
            }
            println!("  bank_digest={}", hex::encode(metadata.bank().digest()));
        }
        "v21" => {
            let metadata =
                noid_miner::v21_artifacts::decode_v21_runtime_metadata_pinned(&bytes, bank)
                    .expect("decode v21 metadata");
            let config = metadata.bank().config();
            println!("v21 pack: {}", metadata_path);
            println!(
                "  schedule: v1_1={:?} v2={:?}",
                config.schedule().v1_1_height(),
                config
                    .schedule()
                    .v2()
                    .map(|a| (a.height(), a.block_time()))
            );
            for class in noid_recursive::acceptance::history_step::v2::capacity_banked::Class::ALL {
                let c = config.class(class);
                println!(
                    "  class wire={} m={} pages={} inputs={} calls={} auth_activation={:?} \
native_headers={:?} native_financial={:?} native_sequential={:?}",
                    class.wire_id(),
                    c.outer_m(),
                    c.pages(),
                    c.max_live_inputs(),
                    c.contract_slots(),
                    c.authorization_activation(),
                    c.native_header_capacity(),
                    c.native_financial_config(),
                    c.native_sequential_config(),
                );
                println!(
                    "    matrix_digest={}",
                    hex::encode(metadata.bank().matrix_digest(class))
                );
            }
            println!("  bank_digest={}", hex::encode(metadata.bank().digest()));
            println!(
                "  v21 flags: fused_pins={} linear_rows={} product_aliases={} shared_m24={}",
                config.fused_pins(),
                config.reuses_linear_rows(),
                config.product_aliases(),
                config.shared_m24()
            );
        }
        other => {
            eprintln!("unknown pack kind {other}");
            std::process::exit(2);
        }
    }
}
