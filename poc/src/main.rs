use std::collections::HashMap;
use std::time::Instant;

use noid_core::hardware::{flat_to_tower_u128, tower_to_flat_u128};
use noid_poseidon2b::native::compression::compress_flat_feed_forward_with_tag;
use noid_poseidon2b::native::domain::{capacity_iv_flat, TAG_CAPSNODE};
use serde_json::json;

fn encode(lanes: [u128; 2]) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[..16].copy_from_slice(&lanes[0].to_le_bytes());
    out[16..].copy_from_slice(&lanes[1].to_le_bytes());
    out
}

fn next_u64(state: &mut u64) -> u64 {
    *state ^= *state >> 12;
    *state ^= *state << 25;
    *state ^= *state >> 27;
    state.wrapping_mul(0x2545_f491_4f6c_dd1d)
}

fn main() {
    let iv = capacity_iv_flat(TAG_CAPSNODE);
    let mut rng = 0x4e4f_4944_434f_4c4cu64;
    let mut seen = HashMap::new();
    let started = Instant::now();

    for evaluations in 1..=1_000_000u64 {
        let subfield = std::array::from_fn::<u128, 4, _>(|_| {
            tower_to_flat_u128((next_u64(&mut rng) & 0xffff) as u128)
        });
        let left = encode([subfield[0], subfield[1]]);
        let right = encode([subfield[2] ^ iv[0], subfield[3] ^ iv[1]]);
        let digest = compress_flat_feed_forward_with_tag(TAG_CAPSNODE, &left, &right);

        assert!(flat_to_tower_u128(u128::from_le_bytes(digest[..16].try_into().unwrap())) <= 0xffff);
        assert!(flat_to_tower_u128(u128::from_le_bytes(digest[16..].try_into().unwrap())) <= 0xffff);

        if let Some((old_left, old_right, old_evaluations)) = seen.insert(digest, (left, right, evaluations)) {
            assert_ne!((old_left, old_right), (left, right));
            assert_eq!(
                digest,
                compress_flat_feed_forward_with_tag(TAG_CAPSNODE, &old_left, &old_right)
            );
            println!(
                "{}",
                json!({
                    "primitive": "compress_flat_feed_forward_with_tag(TAG_CAPSNODE)",
                    "evaluations": evaluations,
                    "elapsed_seconds": started.elapsed().as_secs_f64(),
                    "digest": hex::encode(digest),
                    "first": {
                        "found_at_evaluation": old_evaluations,
                        "left": hex::encode(old_left),
                        "right": hex::encode(old_right),
                    },
                    "second": {
                        "left": hex::encode(left),
                        "right": hex::encode(right),
                    },
                })
            );
            return;
        }
    }

    panic!("no collision found within one million evaluations");
}
