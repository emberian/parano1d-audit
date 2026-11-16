//! Focused local reproductions of the three circuit edge cases from PA-02/03/04.
//! This crate depends only on the pinned source snapshot; it never contacts a node.

use noid_ivc_core::challenger::FsLaneChallenger;
use noid_ivc_core::deep_chain::ff_merkle::{
    build_ff_merkle_path_columns, FfMerklePathFamily, FfMerklePathWitness,
};
use noid_ivc_core::field::F128;
use noid_ivc_core::field_circuit::{
    FieldR1csBuilder, FsChannelOps, FsChannelTrace, FsChannelUnionRecorder, LinExpr, Wire,
};
use noid_ivc_core::pcs::{PcsParams, LOG_PACKING};
use noid_ivc_core::public_io::{PublicIoSpec, WitnessSlice};
use noid_ivc_core::verifier::{verify_field_with_public_io_and_post_commit, VerifyError};
use noid_ivc_prover::field_prover::prove_field_with_public_io_and_post_commit;
use noid_recursive::region_sidecar::{
    verify_merkle_region_sidecar, MerkleRegionFamily, MerkleRegionProverPlan, MerkleRegionVk,
    MERKLE_REGION_COMMITTED_COLUMNS,
};

const DOMAIN: &[u8] = b"verification-circuit-standalone-walk-b-v1";

fn sidecar_nonboolean_direction() {
    // Match the production-shaped 10+6 strided Walk-B fixture already proven
    // honest in noid_recursive's own tests, then change only one active node.
    const W_LOG: usize = 5;
    const N_PATHS: usize = 2;
    const STRIDE: usize = 16;
    const SOURCE_DEPTH: usize = 10;
    const MID_DEPTH: usize = 6;
    let iv = [F128::new(0x1357, 0x2468), F128::new(0x3579, 0x468a)];
    let paths = |depth: usize, salt: u64| {
        (0..N_PATHS)
            .map(|path| FfMerklePathWitness {
                entry: [
                    F128::new(salt + path as u64 + 1, salt + 0x100 + path as u64),
                    F128::new(salt + path as u64 + 3, salt + 0x200 + path as u64),
                ],
                siblings: (0..depth)
                    .map(|level| {
                        [
                            F128::new(
                                salt + 0x300 + (path * depth + level) as u64,
                                salt + 0x400 + level as u64,
                            ),
                            F128::new(
                                salt + 0x500 + (path * depth + level) as u64,
                                salt + 0x600 + level as u64,
                            ),
                        ]
                    })
                    .collect(),
                directions: (0..depth)
                    .map(|level| (path + level + salt as usize) & 1 == 1)
                    .collect(),
            })
            .collect::<Vec<_>>()
    };
    let source_family = FfMerklePathFamily {
        depth: SOURCE_DEPTH,
        n_paths: N_PATHS,
    };
    let mid_family = FfMerklePathFamily {
        depth: MID_DEPTH,
        n_paths: N_PATHS,
    };
    let source =
        build_ff_merkle_path_columns(&source_family, iv, &paths(SOURCE_DEPTH, 0x1000), W_LOG);
    let mid = build_ff_merkle_path_columns(&mid_family, iv, &paths(MID_DEPTH, 0x2000), 4);
    let column_len = 1usize << W_LOG;
    let mut columns: [Vec<F128>; MERKLE_REGION_COMMITTED_COLUMNS] =
        std::array::from_fn(|_| vec![F128::ZERO; column_len]);
    let mut s0: [Vec<F128>; 4] = std::array::from_fn(|_| vec![F128::ZERO; column_len]);
    let mut s_out: [Vec<F128>; 4] = std::array::from_fn(|_| vec![F128::ZERO; column_len]);
    let mut place = |src: &noid_ivc_core::deep_chain::ff_merkle::FfMerklePathColumns,
                     depth: usize,
                     src_stride: usize,
                     offset: usize| {
        for path in 0..N_PATHS {
            for level in 0..depth {
                let from = path * src_stride + level;
                let to = path * STRIDE + offset + level;
                for lane in 0..2 {
                    columns[4 + lane][to] = src.cr[lane][from];
                    columns[6 + lane][to] = src.sib[lane][from];
                }
                columns[8][to] = src.d[from];
                for lane in 0..4 {
                    columns[lane][to] = src.c[lane][from];
                    s0[lane][to] = src.s0[lane][from];
                    s_out[lane][to] = src.s_out[lane][from];
                }
            }
        }
    };
    place(&source, SOURCE_DEPTH, source_family.stride(), 0);
    place(&mid, MID_DEPTH, mid_family.stride(), SOURCE_DEPTH);
    let honest_control = std::env::var_os("HONEST_CONTROL").is_some();
    let d = if honest_control {
        F128::ZERO
    } else {
        F128::new(2, 0)
    };
    let one_plus_d = F128::ONE + d;
    let original_entry = [columns[4][0], columns[5][0]];
    for lane in 0..2 {
        let left = source.cr[lane][0];
        let right = source.sib[lane][0];
        columns[4 + lane][0] = one_plus_d * left + d * right;
        columns[6 + lane][0] = d * left + one_plus_d * right;
        assert_eq!(
            one_plus_d * columns[4 + lane][0] + d * columns[6 + lane][0],
            left
        );
        assert_eq!(
            d * columns[4 + lane][0] + one_plus_d * columns[6 + lane][0],
            right
        );
    }
    columns[8][0] = d;
    let mutated_entry = [columns[4][0], columns[5][0]];
    if !honest_control {
        assert_ne!(mutated_entry, original_entry);
    }

    let mut builder = FieldR1csBuilder::new();
    while builder.num_wires() % 4 != 0 {
        builder.alloc_f128(F128::ZERO);
    }
    let public_io_index = builder.num_wires() / 4;
    let io = [
        mutated_entry[0],
        mutated_entry[1],
        source.roots[0][0],
        source.roots[0][1],
    ];
    let io_wires: [Wire; 4] = io.map(|value| builder.alloc_f128(value));
    while builder.num_wires() % column_len != 0 {
        builder.alloc_f128(F128::ZERO);
    }
    let base = builder.num_wires();
    for column in &columns {
        for &value in column {
            builder.alloc_f128(value);
        }
    }
    let cell =
        |col: usize, slot: usize| LinExpr::from_wire(Wire((base + col * column_len + slot) as u32));
    for lane in 0..2 {
        let entry_eq = cell(4 + lane, 0).add(&LinExpr::from_wire(io_wires[lane]));
        builder.pin_f128(&entry_eq, F128::ZERO);
        let last = SOURCE_DEPTH - 1;
        let cr = cell(4 + lane, last);
        let sib = cell(6 + lane, last);
        let d_last = cell(8, last);
        let mix_product = builder.mul(&d_last, &cr.add(&sib));
        let root_expr = cell(lane, last)
            .add(&cr)
            .add(&LinExpr::from_wire(mix_product))
            .add(&LinExpr::from_wire(io_wires[2 + lane]));
        builder.pin_f128(&root_expr, F128::ZERO);
    }
    if std::env::var_os("PIN_BOOLEAN").is_some() {
        let d_expr = cell(8, 0);
        let invalid = builder.mul(&d_expr, &d_expr.add_const(F128::ONE));
        builder.pin_f128(&LinExpr::from_wire(invalid), F128::ZERO);
    }
    let slices: [WitnessSlice; MERKLE_REGION_COMMITTED_COLUMNS] =
        std::array::from_fn(|column| WitnessSlice {
            log2_len: W_LOG,
            index: base / column_len + column,
        });
    let (r1cs, z) = builder.build();
    if std::env::var_os("PIN_BOOLEAN").is_some() {
        assert!(!r1cs.satisfies(&z));
        println!("PA-02 same public-entry/root witness rejected by outer direction Boolean pin");
        return;
    }
    assert!(r1cs.satisfies(&z));

    // A production-style Boolean pin on the same field value rejects this witness.
    let mut pinned = FieldR1csBuilder::new();
    let d_wire = pinned.alloc_f128(d);
    let d_expr = LinExpr::from_wire(d_wire);
    let bad = pinned.mul(&d_expr, &d_expr.add_const(F128::ONE));
    pinned.pin_f128(&LinExpr::from_wire(bad), F128::ZERO);
    let (pinned_r1cs, pinned_z) = pinned.build();
    assert_eq!(pinned_r1cs.satisfies(&pinned_z), honest_control);

    let vk = MerkleRegionVk::new(
        [0xa2; 32],
        W_LOG,
        slices,
        W_LOG,
        vec![
            MerkleRegionFamily::FeedForwardStrided {
                offset: 0,
                stride: STRIDE,
                depth: SOURCE_DEPTH,
                n_paths: N_PATHS,
                iv,
            },
            MerkleRegionFamily::FeedForwardStrided {
                offset: SOURCE_DEPTH,
                stride: STRIDE,
                depth: MID_DEPTH,
                n_paths: N_PATHS,
                iv,
            },
        ],
    )
    .expect("valid standalone sidecar key");
    let plan =
        MerkleRegionProverPlan::new(&vk, &s0, &s_out).expect("matching permutation endpoints");
    let spec = PublicIoSpec {
        io_slice: WitnessSlice {
            log2_len: 2,
            index: public_io_index,
        },
        io_len: 4,
        claims: Vec::new(),
    };
    let params = PcsParams {
        m: r1cs.m + LOG_PACKING,
        log_inv_rate: 2,
        log_batch_size: 2,
        profile: Default::default(),
    };
    let mut prover_challenger = FsLaneChallenger::new(DOMAIN);
    let (field_proof, sidecar, commitment, _) = prove_field_with_public_io_and_post_commit(
        &r1cs,
        &z,
        &params,
        &spec,
        &io,
        &mut prover_challenger,
        |z, _, challenger| {
            plan.prove(z, challenger)
                .expect("nonboolean sidecar witness")
        },
    );
    let mut verifier_challenger = FsLaneChallenger::new(DOMAIN);
    let result = verify_field_with_public_io_and_post_commit(
        &r1cs,
        &commitment,
        &field_proof,
        &spec,
        &io,
        &sidecar,
        &mut verifier_challenger,
        |proof, challenger| {
            verify_merkle_region_sidecar(&vk, r1cs.m, proof, challenger)
                .map_err(|_| VerifyError::Auxiliary)
        },
    );
    assert!(result.is_ok(), "standalone sidecar result: {result:?}");
    let mut wrong_io = io;
    wrong_io[0] = original_entry[0];
    if !honest_control {
        let mut wrong_io_challenger = FsLaneChallenger::new(DOMAIN);
        let wrong_io_result = verify_field_with_public_io_and_post_commit(
            &r1cs,
            &commitment,
            &field_proof,
            &spec,
            &wrong_io,
            &sidecar,
            &mut wrong_io_challenger,
            |proof, challenger| {
                verify_merkle_region_sidecar(&vk, r1cs.m, proof, challenger)
                    .map_err(|_| VerifyError::Auxiliary)
            },
        );
        assert!(wrong_io_result.is_err());
    }
    println!(
        "PA-02 full standalone FieldR1cs+MerkleRegionSidecar accepted: D={d:?}; public entry {original_entry:?} -> {mutated_entry:?}; public root {:?}; Boolean pin {}",
        source.roots[0],
        if honest_control { "accepts" } else { "rejects" }
    );
}

fn pow_high_half() {
    const BITS: u32 = 16;
    const HI: u64 = 1;
    const DOMAIN: &[u8] = b"verification-circuit-pow-high-half-v1";
    let mut winner = None;
    for lo in 0..(1 << 21) {
        let nonce = F128 { lo, hi: HI };
        let mut builder = FieldR1csBuilder::new();
        let wire = builder.alloc_f128(nonce);
        let mut recorder = FsChannelUnionRecorder::new_c1(DOMAIN);
        recorder.verify_pow(&mut builder, &LinExpr::from_wire(wire), BITS);
        let (r1cs, z) = builder.build();
        if r1cs.satisfies(&z) {
            winner = Some(nonce);
            break;
        }
    }
    let nonce = winner.expect("expected a high-half PoW nonce within 2^21 trials");
    let mut builder = FieldR1csBuilder::new();
    let mut channel = FsChannelTrace::new_c1(&mut builder, DOMAIN);
    let wire = builder.alloc_f128(nonce);
    channel.verify_pow_trace(&mut builder, &LinExpr::from_wire(wire), BITS);
    let (r1cs, z) = builder.build();
    assert!(
        r1cs.satisfies(&z),
        "full inline sponge circuit must accept high-half nonce"
    );

    let mut pinned = FieldR1csBuilder::new();
    let wire = pinned.alloc_f128(nonce);
    pinned.decompose_bits_le(&LinExpr::from_wire(wire), 64);
    let (pinned_r1cs, pinned_z) = pinned.build();
    assert!(!pinned_r1cs.satisfies(&pinned_z));
    println!(
        "PA-03 full inline PoW gadget accepted high-half nonce {nonce:?}; 64-bit range pin rejects"
    );
}

fn main() {
    sidecar_nonboolean_direction();
    pow_high_half();
}
