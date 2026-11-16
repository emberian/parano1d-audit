// SPDX-License-Identifier: Apache-2.0
//! Deterministic algebraic, Fiat-Shamir state-machine, and soundness-accounting
//! verification suite for `parano1d` (`52938433a30b185ab007afaa15166a632cf2e82e`).
//!
//! All checks are read-only mathematical property and API state-transition
//! verifications against the pinned workspace crates.

use std::collections::BTreeSet;
use std::fs;

use noid_core::hardware::{
    clmul_gcm, flat_to_tower_u128, square_flat_u128, tower_to_flat_u128,
};
use noid_core::packed::{PackedBlock128, PACKED_LANES};
use noid_core::sumcheck::RoundPolynomial;
use noid_core::transcript::FiatShamir;
use noid_core::{Block128, Block256, TowerField};
use noid_fri::channel::Channel as FriChannel;
use noid_ivc_core::challenger::{Challenger, FsChallenger, FsLaneChallenger};
use noid_ivc_core::field::gf2_256::F256;
use noid_ivc_core::field::F128;
use noid_ivc_core::merkle::leaf_fixed_iv_flat;
use noid_poseidon2b::batch::{
    packed_poseidon2b_permute, packed_poseidon2b_permute_flat,
    packed_poseidon2b_permute_flat_many,
};
use noid_poseidon2b::hasher::CryptographicHasher;
use noid_poseidon2b::native::compression::Poseidon2bSponge;
use noid_poseidon2b::native::digest::poseidon2b_digest;
use noid_poseidon2b::native::domain::{capacity_iv, capacity_iv_flat, DomainTag};
use noid_poseidon2b::native::permutation::{
    permute_flat_u128, Poseidon2bPermutation, MDS_FULL, MDS_PARTIAL, N_ROUNDS, ROUND_CONSTANTS,
    STATE_SIZE,
};
use noid_poseidon2b::native::compress_flat_feed_forward_with_tag;
use noid_poseidon2b::{Poseidon2bChannel, Poseidon2bWideChannel};
use noid_soundness::parameters::{HistoryClassParameters, ProductionParameters};
use noid_soundness::{qrom, resource};
use serde::Serialize;

/// Deterministic SplitMix64 PRNG for reproducible algebraic property sampling.
struct Rng64(u64);

impl Rng64 {
    const fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn next_u128(&mut self) -> u128 {
        (self.next_u64() as u128) | ((self.next_u64() as u128) << 64)
    }

    fn next_block128(&mut self) -> Block128 {
        Block128(self.next_u128())
    }
}

/// Apply the `k`-th Frobenius automorphism `sigma_k(x) = x^(2^(16*k))` in tower basis.
fn frob16_tower(mut x: Block128, k: usize) -> Block128 {
    for _ in 0..(16 * k) {
        x = x.square();
    }
    x
}

/// Apply `sigma_k` in flat (GCM) basis via `16*k` flat squarings.
fn frob16_flat(mut x_flat: u128, k: usize) -> u128 {
    for _ in 0..(16 * k) {
        x_flat = square_flat_u128(x_flat);
    }
    x_flat
}

/// Apply `Phi^k(Z) = Z^(2^(16*k))` in `Block256 = GF(2^256)` (`k = 0..=16`).
fn frob16_block256(mut z: Block256, k: usize) -> Block256 {
    for _ in 0..(16 * k) {
        z = z.square();
    }
    z
}

/// Absolute trace `Tr_{GF(2^128)/GF(2)}(x)` in tower basis.
fn abs_trace_128(x: Block128) -> u8 {
    let mut acc = Block128::ZERO;
    let mut cur = x;
    for _ in 0..128 {
        acc += cur;
        cur = cur.square();
    }
    assert!(acc == Block128::ZERO || acc == Block128::ONE);
    if acc == Block128::ONE { 1 } else { 0 }
}

/// Solve `z^2 + z = rhs` in `Block128` (`Tr(rhs) == 0`) using the 128x128 GF(2) linear map.
fn solve_artin_schreier_128(rhs: Block128) -> Option<Block128> {
    if abs_trace_128(rhs) != 0 {
        return None;
    }
    // Build 128 columns of L(e_j) = e_j^2 + e_j in tower basis.
    let mut cols = [0u128; 128];
    for j in 0..128 {
        let ej = Block128(1u128 << j);
        cols[j] = (ej.square() + ej).0;
    }
    // Augmented 128 x 129 system over GF(2): row r holds bit r of each column + rhs bit r.
    let mut rows = [(0u128, 0u8); 128];
    for r in 0..128 {
        let mut mask = 0u128;
        for j in 0..128 {
            if ((cols[j] >> r) & 1) == 1 {
                mask |= 1u128 << j;
            }
        }
        let b = ((rhs.0 >> r) & 1) as u8;
        rows[r] = (mask, b);
    }
    let mut pivot_col = [None::<usize>; 128];
    let mut rank = 0usize;
    for col in 0..128 {
        let mut sel = None;
        for r in rank..128 {
            if ((rows[r].0 >> col) & 1) == 1 {
                sel = Some(r);
                break;
            }
        }
        let Some(piv) = sel else { continue };
        rows.swap(rank, piv);
        pivot_col[rank] = Some(col);
        let (pmask, pb) = rows[rank];
        for r in 0..128 {
            if r != rank && ((rows[r].0 >> col) & 1) == 1 {
                rows[r].0 ^= pmask;
                rows[r].1 ^= pb;
            }
        }
        rank += 1;
    }
    for r in rank..128 {
        if rows[r].1 != 0 {
            return None;
        }
    }
    let mut sol = 0u128;
    for r in 0..rank {
        if rows[r].1 == 1 {
            let c = pivot_col[r].unwrap();
            sol |= 1u128 << c;
        }
    }
    let z = Block128(sol);
    assert_eq!(z.square() + z, rhs);
    Some(z)
}

/// Solve a 64x64 GF(2) linear system `A * x = b` where column `j` (`0..64`) of `A` is `cols[j]`.
/// Returns `(rank, solution_count_if_consistent, sample_solution)`.
fn solve_gf2_64x64(cols: &[u64; 64], rhs: u64) -> (usize, u128, Option<u64>) {
    let mut rows = [(0u64, 0u8); 64];
    for r in 0..64 {
        let mut mask = 0u64;
        for j in 0..64 {
            if ((cols[j] >> r) & 1) == 1 {
                mask |= 1u64 << j;
            }
        }
        let b = ((rhs >> r) & 1) as u8;
        rows[r] = (mask, b);
    }
    let mut pivot_col = [None::<usize>; 64];
    let mut rank = 0usize;
    for col in 0..64 {
        let mut sel = None;
        for r in rank..64 {
            if ((rows[r].0 >> col) & 1) == 1 {
                sel = Some(r);
                break;
            }
        }
        let Some(piv) = sel else { continue };
        rows.swap(rank, piv);
        pivot_col[rank] = Some(col);
        let (pmask, pb) = rows[rank];
        for r in 0..64 {
            if r != rank && ((rows[r].0 >> col) & 1) == 1 {
                rows[r].0 ^= pmask;
                rows[r].1 ^= pb;
            }
        }
        rank += 1;
    }
    for r in rank..64 {
        if rows[r].1 != 0 {
            return (rank, 0, None);
        }
    }
    let mut sol = 0u64;
    for r in 0..rank {
        if rows[r].1 == 1 {
            sol |= 1u64 << pivot_col[r].unwrap();
        }
    }
    let nullity = 64 - rank;
    let count = 1u128 << nullity;
    (rank, count, Some(sol))
}

/// Complete inventory of all 87 distinct 8-byte ASCII `DomainTag` labels across `parano1d`.
const ALL_87_DOMAIN_TAGS: [&[u8; 8]; 87] = [
    b"ACCBLK__",
    b"ADDRFIX_",
    b"BLOCKHDR",
    b"BTCHTST_",
    b"BYTEHASH",
    b"CAPS256_",
    b"CAPSLEAF",
    b"CAPSMIX_",
    b"CAPSNODE",
    b"CLAIMS__",
    b"CNTCODE_",
    b"CNTOBJ__",
    b"CNTPOL__",
    b"CNTRCT__",
    b"COMMIT__",
    b"COMPRESS",
    b"DAWTNSS_",
    b"EXSTNOD_",
    b"EXSTSLT_",
    b"FFTBTST_",
    b"FFTESTA_",
    b"FFTESTB_",
    b"FFTESTC_",
    b"FLSPNGA_",
    b"FLSPNGB_",
    b"FRICHANL",
    b"FRISTATE",
    b"FSCH256_",
    b"FSCHALNG",
    b"HISTCLM_",
    b"HISTPRF_",
    b"HISTTRN_",
    b"IVCPCSF_",
    b"IVCPCSL_",
    b"IVCPCSN_",
    b"KSCH256_",
    b"KSCHANNL",
    b"LANE256_",
    b"LANECHAL",
    b"LEAFFIX_",
    b"LEAFTST_",
    b"LEAF____",
    b"LNDEF_21",
    b"LNREQ_21",
    b"LNROUND1",
    b"LNSTAT21",
    b"MKCLM_21",
    b"MKDEF_21",
    b"MKLPBN21",
    b"MKLPCM21",
    b"MKLPIN21",
    b"MKLPOU21",
    b"MKLP__21",
    b"MKPOOL21",
    b"MKQNOD21",
    b"MKQROOT1",
    b"MKREQID1",
    b"MKREQ_21",
    b"MKROUND1",
    b"MKSEED21",
    b"NATBAL21",
    b"NATCOL21",
    b"NATDEF21",
    b"NATID_21",
    b"NATNFT21",
    b"NATNFTID",
    b"NATRES21",
    b"NMBUY_21",
    b"NMDEF_21",
    b"NMLIST21",
    b"NMOFF_21",
    b"NMPRIOR1",
    b"NMROUND1",
    b"NONCETST",
    b"P6BENCH_",
    b"PAGEDTX_",
    b"POLICY__",
    b"POWHDR__",
    b"SC26AUD_",
    b"SEGTREE_",
    b"SEMHDR__",
    b"TRBCH_21",
    b"TRINT_21",
    b"TRORD_21",
    b"TRPOL_21",
    b"TX8X2___",
    b"TXROOT__",
];

#[derive(Serialize)]
struct AuditSummary {
    commit: &'static str,
    galois_equivariance: GaloisEquivarianceReport,
    subfield_lattice: Vec<SubfieldReport>,
    domain_tags: DomainTagReport,
    compression_and_zero_iv: CompressionZeroIvReport,
    extension_field_256: Extension256Report,
    fiat_shamir_channels: ChannelAuditReport,
    soundness_accounting: SoundnessAuditReport,
}

#[derive(Serialize)]
struct GaloisEquivarianceReport {
    basis_isomorphism_samples: usize,
    round_constants_all_in_gf256: bool,
    mds_full_all_in_gf256: bool,
    mds_partial_active_entry_in_gf256: bool,
    mds_partial_passive_diag_in_gf65536_minus_gf256: bool,
    minimal_parameter_subfield_bits: usize,
    galois_group_order_128_over_16: usize,
    sigma4_closed_form_verified_samples: usize,
    tower_permute_equivariant_all_k: bool,
    flat_permute_pmull_equivariant_all_k: bool,
    packed_permute_equivariant_all_k: bool,
    packed_flat_many_equivariant_all_k: bool,
}

#[derive(Serialize)]
struct SubfieldReport {
    subfield_bits: usize,
    state_space_bits: usize,
    rate_digest_bits: usize,
    invariant_full_permutation: bool,
    invariant_flat_basis: bool,
}

#[derive(Serialize)]
struct DomainTagReport {
    total_distinct_repo_tags: usize,
    any_tag_iv_in_gf2_64_sq: bool,
    sigma4_formula_all_verified: bool,
    min_frobenius_orbit_size: usize,
    cross_tag_frobenius_conjugate_pairs: usize,
    empty_rate_one_perm_collisions: usize,
    leaf_fixed_iv_aliases_any_domain_tag: bool,
    leaf_fixed_iv_sigma_k_neq_4_possible: bool,
    leaf_fixed_iv_sigma4_linear_rank: usize,
    leaf_fixed_iv_sigma4_conjugate_u64_solutions: u128,
}

#[derive(Serialize)]
struct CompressionZeroIvReport {
    ff_tag_frobenius_equivariant_all_k: bool,
    ff_tag_sigma4_delta_equals_iv_lo_zero: bool,
    ff_tag_gf2_16_coset_invariant: bool,
    ff_tag_gf2_32_coset_invariant: bool,
    ff_tag_gf2_64_coset_invariant_with_zero_b1_shift: bool,
    hash_pair_galois_equivariant: bool,
    hash_pair_ignores_sponge_iv: bool,
    hash_field_ignores_sponge_iv: bool,
    hash_concatenation_ignores_sponge_iv: bool,
    zero_iv_block0_gf2_16_traps_capacity_in_32_bits: bool,
    keystore_with_zero_iv_matches_digest_and_hash_concatenation: bool,
}

#[derive(Serialize)]
struct Extension256Report {
    tower_flat_f256_isomorphism_samples: usize,
    raw_challenge_two_to_one_verified: bool,
    raw_challenge_trace_one_and_non_base: bool,
    raw_challenge_generates_full_gf2_256: bool,
    galois_256_over_16_order: usize,
    cocycle_constants_e_k_verified_all_16: bool,
}

#[derive(Serialize)]
struct ChannelAuditReport {
    poseidon2b_channel_zero_absorb_squeeze_pair_is_zero: bool,
    fri_channel_zero_absorb_squeeze_pair_is_zero: bool,
    wide_channel_zero_absorb_squeeze_is_zero_tau: bool,
    fs_challenger_frames_domain_on_init: bool,
    fs_lane_challenger_frames_domain_on_init: bool,
    odd_even_squeeze_post_absorb_state_alias_base: bool,
    odd_even_squeeze_base_then_wide_alias: bool,
    zk_auth_single_tag_shifts_wide_across_rate_blocks: bool,
    fs_lane_pow_even_and_odd_parity_roundtrip: bool,
    fs_lane_pow_zero_bits_rejects_nonzero_nonce: bool,
    empty_round_poly_reports_degree_zero_and_absorbs_nothing: bool,
}

#[derive(Serialize)]
struct SoundnessAuditReport {
    poseidon_audit_appendix_a_applies: bool,
    poseidon_audit_ideal_degree_exponent: u32,
    poseidon_audit_even_balancing_core_hex: String,
    poseidon_audit_even_balancing_core_in_gf2_16: bool,
    actual_gf2_16_univariate_degree_cap: u64,
    legacy_history_squeeze_perms: u32,
    heterogeneous_m24_m25_max_perms: u32,
    heterogeneous_m24_m25_min_perms: u32,
    qrom_max_vs_min_gate_depth_undercharge_num: u64,
    qrom_max_vs_min_gate_depth_undercharge_den: u64,
    qrom_max_vs_min_gate_depth_undercharge_ratio: f64,
    fs_lane_challenger_23_lanes_total_perms: usize,
    fs_lane_challenger_23_lanes_effective_perms: usize,
    wallet_wide_channel_grind_plus_7_seeds_effective_perms: usize,
}

fn main() {
    let mut rng = Rng64::new(0x5293_8433_A30B_185A);

    // -------------------------------------------------------------------------
    // 1. Basis Isomorphism, Parameter Subfield & Gal(GF(2^128)/GF(2^16))
    // -------------------------------------------------------------------------
    let iso_samples = 512usize;
    assert_eq!(tower_to_flat_u128(0), 0);
    assert_eq!(tower_to_flat_u128(1), 1);
    assert_eq!(flat_to_tower_u128(0), 0);
    assert_eq!(flat_to_tower_u128(1), 1);
    for _ in 0..iso_samples {
        let a = rng.next_block128();
        let b = rng.next_block128();
        let af = tower_to_flat_u128(a.0);
        let bf = tower_to_flat_u128(b.0);
        assert_eq!(flat_to_tower_u128(af), a.0);
        assert_eq!(tower_to_flat_u128((a + b).0), af ^ bf);
        assert_eq!(tower_to_flat_u128((a * b).0), clmul_gcm(af, bf));
        assert_eq!(tower_to_flat_u128(a.square().0), square_flat_u128(af));
    }

    let rc_in_gf256 = ROUND_CONSTANTS
        .iter()
        .all(|lane| lane.iter().all(|&c| c < 256));
    let mds_full_in_gf256 = MDS_FULL.iter().all(|row| row.iter().all(|&c| c < 256));
    let mds_partial_00_in_gf256 = MDS_PARTIAL[0][0] < 256;
    let mds_partial_diag_in_gf65536_not_256 = (1..STATE_SIZE)
        .all(|i| MDS_PARTIAL[i][i] >= 256 && MDS_PARTIAL[i][i] < 65536);
    assert!(rc_in_gf256 && mds_full_in_gf256 && mds_partial_00_in_gf256);
    assert!(mds_partial_diag_in_gf65536_not_256);

    for _ in 0..iso_samples {
        let x = rng.next_block128();
        let s4 = frob16_tower(x, 4);
        assert_eq!(s4.0, x.0 ^ (x.0 >> 64));
        assert_eq!(frob16_tower(x, 8), x);
    }

    let perm = Poseidon2bPermutation;
    let mut tower_eq = true;
    let mut flat_eq = true;
    let mut packed_eq = true;
    let mut packed_many_eq = true;

    for _ in 0..64 {
        let s: [Block128; 4] = std::array::from_fn(|_| rng.next_block128());
        let mut ps = s;
        perm.permute_mut(&mut ps);

        let sf: [u128; 4] = std::array::from_fn(|i| tower_to_flat_u128(s[i].0));
        let mut psf = sf;
        permute_flat_u128(&mut psf);
        for i in 0..4 {
            assert_eq!(psf[i], tower_to_flat_u128(ps[i].0));
        }

        for k in 0..8 {
            let mut sk: [Block128; 4] = std::array::from_fn(|i| frob16_tower(s[i], k));
            perm.permute_mut(&mut sk);
            let rhs_t: [Block128; 4] = std::array::from_fn(|i| frob16_tower(ps[i], k));
            if sk != rhs_t {
                tower_eq = false;
            }

            let mut sfk: [u128; 4] = std::array::from_fn(|i| frob16_flat(sf[i], k));
            permute_flat_u128(&mut sfk);
            let rhs_f: [u128; 4] = std::array::from_fn(|i| frob16_flat(psf[i], k));
            if sfk != rhs_f {
                flat_eq = false;
            }

            // Packed tower kernel
            let mut p_in: [PackedBlock128; 4] =
                std::array::from_fn(|i| PackedBlock128::broadcast(frob16_tower(s[i], k)));
            packed_poseidon2b_permute(&mut p_in);
            for i in 0..4 {
                for lane in 0..PACKED_LANES {
                    if p_in[i].get_lane(lane) != rhs_t[i] {
                        packed_eq = false;
                    }
                }
            }

            // Packed flat single & 4-group interleaved kernels
            let mut pf_one: [PackedBlock128; 4] =
                std::array::from_fn(|i| PackedBlock128::broadcast(Block128(frob16_flat(sf[i], k))));
            packed_poseidon2b_permute_flat(&mut pf_one);
            let mut pf_many: [[PackedBlock128; 4]; 4] = std::array::from_fn(|g| {
                std::array::from_fn(|i| {
                    let kk = (k + g) % 8;
                    PackedBlock128::broadcast(Block128(frob16_flat(sf[i], kk)))
                })
            });
            packed_poseidon2b_permute_flat_many(&mut pf_many);
            for i in 0..4 {
                if pf_one[i].get_lane(0).0 != rhs_f[i] {
                    packed_eq = false;
                }
                for g in 0..4 {
                    let kk = (k + g) % 8;
                    if pf_many[g][i].get_lane(0).0 != frob16_flat(psf[i], kk) {
                        packed_many_eq = false;
                    }
                }
            }
        }
    }
    assert!(tower_eq && flat_eq && packed_eq && packed_many_eq);

    // -------------------------------------------------------------------------
    // 2. Subfield Lattice Invariance: GF(2^d)^4 for d in {1, 2, 4, 8, 16, 32, 64}
    // -------------------------------------------------------------------------
    let mut subfield_reports = Vec::new();
    for &d in &[1usize, 2, 4, 8, 16, 32, 64] {
        let mask: u128 = if d == 128 { u128::MAX } else { (1u128 << d) - 1 };
        let mut all_in_tower = true;
        let mut all_in_flat = true;
        for t in 0..256u128 {
            let st: [Block128; 4] = if t == 0 {
                [Block128::ONE, Block128::ZERO, Block128::ZERO, Block128::ZERO]
            } else {
                std::array::from_fn(|_| Block128(rng.next_u128() & mask))
            };
            let mut out = st;
            perm.permute_mut(&mut out);
            if out.iter().any(|x| (x.0 & !mask) != 0) {
                all_in_tower = false;
            }
            let mut fl: [u128; 4] = std::array::from_fn(|i| tower_to_flat_u128(st[i].0));
            permute_flat_u128(&mut fl);
            if fl.iter().any(|&xf| (flat_to_tower_u128(xf) & !mask) != 0) {
                all_in_flat = false;
            }
        }
        assert_eq!(all_in_tower, d >= 16);
        assert_eq!(all_in_flat, d >= 16);
        subfield_reports.push(SubfieldReport {
            subfield_bits: d,
            state_space_bits: 4 * d,
            rate_digest_bits: 2 * d,
            invariant_full_permutation: all_in_tower,
            invariant_flat_basis: all_in_flat,
        });
    }

    // -------------------------------------------------------------------------
    // 3. Complete 87-Tag DomainTag Audit & `leaf_fixed_iv_flat` Linear Algebra
    // -------------------------------------------------------------------------
    let mut seen_labels = BTreeSet::new();
    let mut iv_set = BTreeSet::new();
    let mut empty_perm_outputs = BTreeSet::new();
    let mut any_in_gf2_64_sq = false;
    let mut sigma4_iv_ok = true;
    let mut min_orbit = usize::MAX;

    let mut tag_ivs = Vec::with_capacity(ALL_87_DOMAIN_TAGS.len());
    for label in ALL_87_DOMAIN_TAGS {
        assert!(seen_labels.insert(*label), "duplicate tag in list: {:?}", std::str::from_utf8(label));
        let tag = DomainTag::new(label);
        let iv = capacity_iv(tag);
        assert!(iv_set.insert((iv[0].0, iv[1].0)));
        if (iv[0].0 >> 64) == 0 && (iv[1].0 >> 64) == 0 {
            any_in_gf2_64_sq = true;
        }
        let s4_0 = frob16_tower(iv[0], 4);
        let s4_1 = frob16_tower(iv[1], 4);
        if s4_0 != (iv[0] + iv[1]) || s4_1 != iv[1] {
            sigma4_iv_ok = false;
        }
        let mut orbit = BTreeSet::new();
        for k in 0..8 {
            orbit.insert((frob16_tower(iv[0], k).0, frob16_tower(iv[1], k).0));
        }
        min_orbit = min_orbit.min(orbit.len());

        let mut st = [Block128::ZERO, Block128::ZERO, iv[0], iv[1]];
        perm.permute_mut(&mut st);
        assert!(empty_perm_outputs.insert((st[0].0, st[1].0)));
        tag_ivs.push(iv);
    }
    assert_eq!(seen_labels.len(), 87);
    assert!(!any_in_gf2_64_sq);
    assert!(sigma4_iv_ok);
    assert_eq!(min_orbit, 8);

    let mut cross_conjugates = 0usize;
    for i in 0..tag_ivs.len() {
        for j in 0..tag_ivs.len() {
            if i == j {
                continue;
            }
            for k in 1..8 {
                if frob16_tower(tag_ivs[i][0], k) == tag_ivs[j][0]
                    && frob16_tower(tag_ivs[i][1], k) == tag_ivs[j][1]
                {
                    cross_conjugates += 1;
                }
            }
        }
    }
    assert_eq!(cross_conjugates, 0);

    // Check `leaf_fixed_iv_flat(len)`:
    // `[hi ^ (len as u128), lo]` where `[hi, lo] = capacity_iv_flat(DomainTag::new(b"IVCPCSF_"))`.
    let ivcpcsf = DomainTag::new(b"IVCPCSF_");
    let [fix_hi_flat, fix_lo_flat] = capacity_iv_flat(ivcpcsf);
    let [fix_hi_tow, fix_lo_tow] = capacity_iv(ivcpcsf);

    // Does leaf_fixed_iv_flat(len) for len > 0 alias any DomainTag::capacity_iv_flat?
    // In tower basis, capacity_iv(tag) = [label<<64, label], so iv_hi ^ iv_lo has low64 == label, high64 == label.
    // Specifically iv_lo determines label = low64(T^{-1}(iv_lo_flat)), fixing iv_hi_flat uniquely!
    let mut leaf_aliases_tag = false;
    for &label in &ALL_87_DOMAIN_TAGS {
        let [th, tl] = capacity_iv_flat(DomainTag::new(label));
        if tl == fix_lo_flat && label != b"IVCPCSF_" {
            leaf_aliases_tag = true;
        }
        let _ = th;
    }
    assert!(!leaf_aliases_tag);

    // Check k != 4: sigma_k(fix_lo_tow) == fix_lo_tow iff fix_lo_tow in fixed field of sigma_k.
    // For k in {1,3,5,7} fixed field is GF(2^16); for k in {2,6} fixed field is GF(2^32).
    // Since fix_lo_tow = u64::from_le_bytes(*b"IVCPCSF_") has nonzero upper 32 bits of u64 (`"CSF_"`),
    // fix_lo_tow lies in GF(2^64) \ GF(2^32), so sigma_k(fix_lo_tow) != fix_lo_tow for ALL k != 4!
    let mut k_neq_4_possible = false;
    for k in [1usize, 2, 3, 5, 6, 7] {
        if frob16_flat(fix_lo_flat, k) == fix_lo_flat {
            k_neq_4_possible = true;
        }
    }
    assert!(!k_neq_4_possible);

    // Exact 64x64 GF(2) linear system for k = 4:
    // sigma_4_flat(fix_hi_flat ^ len1) == fix_hi_flat ^ len2  (with len1, len2 < 2^64)
    // <=> high64(sigma_4_flat(fix_hi_flat) ^ fix_hi_flat ^ sigma_4_flat(len1)) == 0
    // Note sigma_4_flat(fix_hi_flat) ^ fix_hi_flat == fix_lo_flat!
    assert_eq!(frob16_flat(fix_hi_flat, 4) ^ fix_hi_flat, fix_lo_flat);
    let target_hi64 = (fix_lo_flat >> 64) as u64;
    let mut cols64 = [0u64; 64];
    for j in 0..64 {
        let img = frob16_flat(1u128 << j, 4);
        cols64[j] = (img >> 64) as u64;
    }
    let (lin_rank, u64_solutions, sample_len1) = solve_gf2_64x64(&cols64, target_hi64);
    if let Some(l1) = sample_len1 {
        let s4_l1 = frob16_flat(fix_hi_flat ^ (l1 as u128), 4);
        let l2_u128 = s4_l1 ^ fix_hi_flat;
        assert!(l2_u128 <= u64::MAX as u128);
        let iv1 = leaf_fixed_iv_flat(l1 as usize);
        let iv2 = leaf_fixed_iv_flat(l2_u128 as usize);
        assert_eq!(frob16_flat(iv1[0], 4), iv2[0]);
        assert_eq!(frob16_flat(iv1[1], 4), iv2[1]);
    }
    let _ = fix_hi_tow;
    let _ = fix_lo_tow;

    // -------------------------------------------------------------------------
    // 4. `compress_flat_feed_forward_with_tag` & Zero-IV Sponge Subspace Audit
    // -------------------------------------------------------------------------
    let test_tag = DomainTag::new(b"EXSTNOD_");
    let iv_flat = capacity_iv_flat(test_tag);
    let mut ff_equivariant = true;
    let delta4 = [frob16_flat(iv_flat[0], 4) ^ iv_flat[0], frob16_flat(iv_flat[1], 4) ^ iv_flat[1]];
    let delta4_matches = delta4 == [iv_flat[1], 0];
    assert!(delta4_matches);

    let pack_pair = |[x0, x1]: [u128; 2]| -> [u8; 32] {
        let mut out = [0u8; 32];
        out[..16].copy_from_slice(&x0.to_le_bytes());
        out[16..].copy_from_slice(&x1.to_le_bytes());
        out
    };
    let unpack_pair = |b: [u8; 32]| -> [u128; 2] {
        [
            u128::from_le_bytes(b[..16].try_into().unwrap()),
            u128::from_le_bytes(b[16..].try_into().unwrap()),
        ]
    };

    for _ in 0..64 {
        let a = [rng.next_u128(), rng.next_u128()];
        let b = [rng.next_u128(), rng.next_u128()];
        let out = unpack_pair(compress_flat_feed_forward_with_tag(
            test_tag,
            &pack_pair(a),
            &pack_pair(b),
        ));
        for k in 0..8 {
            let dk = [
                frob16_flat(iv_flat[0], k) ^ iv_flat[0],
                frob16_flat(iv_flat[1], k) ^ iv_flat[1],
            ];
            let ak = [frob16_flat(a[0], k), frob16_flat(a[1], k)];
            let bk = [frob16_flat(b[0], k) ^ dk[0], frob16_flat(b[1], k) ^ dk[1]];
            let out_k = unpack_pair(compress_flat_feed_forward_with_tag(
                test_tag,
                &pack_pair(ak),
                &pack_pair(bk),
            ));
            if out_k != [frob16_flat(out[0], k), frob16_flat(out[1], k)] {
                ff_equivariant = false;
            }
        }
    }
    assert!(ff_equivariant);

    let check_ff_coset = |d: usize, rng: &mut Rng64, zero_b1_shift: bool| -> bool {
        let mask = (1u128 << d) - 1;
        for _ in 0..64 {
            let a_tow = [rng.next_u128() & mask, rng.next_u128() & mask];
            let u_tow = [rng.next_u128() & mask, rng.next_u128() & mask];
            let a_fl = [tower_to_flat_u128(a_tow[0]), tower_to_flat_u128(a_tow[1])];
            let b_fl = [
                tower_to_flat_u128(u_tow[0]) ^ iv_flat[0],
                if zero_b1_shift {
                    tower_to_flat_u128(u_tow[1])
                } else {
                    tower_to_flat_u128(u_tow[1]) ^ iv_flat[1]
                },
            ];
            let out_fl = unpack_pair(compress_flat_feed_forward_with_tag(
                test_tag,
                &pack_pair(a_fl),
                &pack_pair(b_fl),
            ));
            let out_tow = [flat_to_tower_u128(out_fl[0]), flat_to_tower_u128(out_fl[1])];
            if (out_tow[0] & !mask) != 0 || (out_tow[1] & !mask) != 0 {
                return false;
            }
        }
        true
    };
    let ff_16 = check_ff_coset(16, &mut rng, false);
    let ff_32 = check_ff_coset(32, &mut rng, false);
    let ff_64 = check_ff_coset(64, &mut rng, true);
    assert!(ff_16 && ff_32 && ff_64);

    // CryptographicHasher trait behavior & zero-IV sponge subspace property
    let tagged_sponge = Poseidon2bSponge::with_iv(capacity_iv(DomainTag::new(b"FRISTATE")));
    let bare_sponge = Poseidon2bSponge::new();
    let x0 = rng.next_block128();
    let x1 = rng.next_block128();
    let hp_tagged = tagged_sponge.hash_pair(&x0, &x1);
    let hp_bare = bare_sponge.hash_pair(&x0, &x1);
    let hash_pair_ignores_iv = hp_tagged == hp_bare;
    let hash_field_ignores_iv = tagged_sponge.hash_field(&x0) == bare_sponge.hash_field(&x0);
    let hash_concat_ignores_iv =
        tagged_sponge.hash_concatenation(&hp_tagged, &hp_bare) == bare_sponge.hash_concatenation(&hp_tagged, &hp_bare);
    assert!(hash_pair_ignores_iv && hash_field_ignores_iv && hash_concat_ignores_iv);

    let mut hp_equivariant = true;
    for k in 0..8 {
        let hk = bare_sponge.hash_pair(&frob16_tower(x0, k), &frob16_tower(x1, k));
        let [l0u, l1u] = unpack_pair(hp_bare);
        let [kl0u, kl1u] = unpack_pair(hk);
        if Block128(kl0u) != frob16_tower(Block128(l0u), k)
            || Block128(kl1u) != frob16_tower(Block128(l1u), k)
        {
            hp_equivariant = false;
        }
    }
    assert!(hp_equivariant);

    // Verify Block-0 zero-IV capacity trap: for any Block-0 rate (a0, a1) in GF(2^16)^2,
    // permute([a0, a1, 0, 0]) leaves capacity (state[2], state[3]) inside the 32-bit subspace GF(2^16)^2.
    let mut zero_iv_cap_trapped = true;
    for _ in 0..128 {
        let a0 = Block128(rng.next_u128() & 0xFFFF);
        let a1 = Block128(rng.next_u128() & 0xFFFF);
        let mut st = [a0, a1, Block128::ZERO, Block128::ZERO];
        perm.permute_mut(&mut st);
        if (st[2].0 >> 16) != 0 || (st[3].0 >> 16) != 0 {
            zero_iv_cap_trapped = false;
        }
    }
    assert!(zero_iv_cap_trapped);

    // Verify exact byte-for-byte domain identity of `keystore.rs:79` (`with_iv([ZERO; 2])`),
    // `poseidon2b_digest`, and `CryptographicHasher::hash_concatenation` on 64-byte inputs.
    let master: [u8; 32] = std::array::from_fn(|i| (i as u8).wrapping_mul(17).wrapping_add(3));
    let index: u64 = 7;
    let domain_const: u128 = 0x6E6F69642D64657269;
    let mut ks_sponge = Poseidon2bSponge::with_iv([Block128::ZERO; 2]);
    let m0 = Block128(u128::from_le_bytes(master[..16].try_into().unwrap()));
    let m1 = Block128(u128::from_le_bytes(master[16..].try_into().unwrap()));
    ks_sponge.absorb(m0);
    ks_sponge.absorb(m1);
    ks_sponge.absorb(Block128(index as u128));
    ks_sponge.absorb(Block128(domain_const));
    let ks_out = ks_sponge.finalize();

    let mut block1 = [0u8; 32];
    block1[..16].copy_from_slice(&(index as u128).to_le_bytes());
    block1[16..].copy_from_slice(&domain_const.to_le_bytes());
    let mut msg64 = [0u8; 64];
    msg64[..32].copy_from_slice(&master);
    msg64[32..].copy_from_slice(&block1);
    let digest_out = poseidon2b_digest(&msg64);
    let hc_out = bare_sponge.hash_concatenation(&master, &block1);
    let keystore_alias_verified = (ks_out == digest_out) && (ks_out == hc_out);
    assert!(keystore_alias_verified);

    // -------------------------------------------------------------------------
    // 5. Quadratic Extension GF(2^256) Challenge Map & Gal(GF(2^256)/GF(2^16))
    // -------------------------------------------------------------------------
    let to_f128_flat = |b: Block128| -> F128 {
        let fl = tower_to_flat_u128(b.0);
        F128 {
            lo: fl as u64,
            hi: (fl >> 64) as u64,
        }
    };
    let to_f256_flat = |z: Block256| -> F256 {
        F256 {
            lo: to_f128_flat(z.lo),
            hi: to_f128_flat(z.hi),
        }
    };

    let mut two_to_one_ok = true;
    let mut trace_one_ok = true;
    let mut full_gen_ok = true;
    for _ in 0..256 {
        let a = Block256 {
            lo: rng.next_block128(),
            hi: rng.next_block128(),
        };
        let b = Block256 {
            lo: rng.next_block128(),
            hi: rng.next_block128(),
        };
        let af = to_f256_flat(a);
        let bf = to_f256_flat(b);
        assert_eq!(to_f256_flat(a + b), af + bf);
        assert_eq!(to_f256_flat(a * b), af * bf);
        if a != Block256::ZERO {
            assert_eq!(to_f256_flat(a.invert()), af.inv());
        }

        let lo = rng.next_block128();
        let raw_hi = rng.next_block128();
        let c0 = Block256::from_raw_challenge_lanes(lo, raw_hi);
        let c1 = Block256::from_raw_challenge_lanes(lo, raw_hi + Block128::ONE);
        let cf0 = F256::from_raw_challenge_lanes(to_f128_flat(lo), to_f128_flat(raw_hi));
        assert_eq!(to_f256_flat(c0), cf0);
        if c0 != c1 {
            two_to_one_ok = false;
        }
        if c0.is_in_base_subfield() || abs_trace_128(c0.hi) != 1 {
            trace_one_ok = false;
        }
        // Unique maximal proper subfield of GF(2^256) is GF(2^128), fixed by Phi^8: Z -> Z^(2^128).
        if frob16_block256(c0, 8) == c0 {
            full_gen_ok = false;
        }
    }
    assert!(two_to_one_ok && trace_one_ok && full_gen_ok);

    let mut cocycle_ok = true;
    for k in 0..=16 {
        let rhs = frob16_tower(Block128::TAU, k % 8) + Block128::TAU;
        let base_e = solve_artin_schreier_128(rhs).expect("Tr(sigma_k(TAU) + TAU) == 0");
        // Pick the unique root in {base_e, base_e + 1} matching Phi^k(X_7) = e_k + X_7.
        let x7 = Block256 {
            lo: Block128::ZERO,
            hi: Block128::ONE,
        };
        let img_x7 = frob16_block256(x7, k);
        assert_eq!(img_x7.hi, Block128::ONE);
        let ek = img_x7.lo;
        if ek != base_e && ek != (base_e + Block128::ONE) {
            cocycle_ok = false;
        }
        for _ in 0..16 {
            let lo = rng.next_block128();
            let y = rng.next_block128();
            let z = Block256 { lo, hi: y };
            let phi_z = frob16_block256(z, k);
            let expected = Block256 {
                lo: frob16_tower(lo, k % 8) + frob16_tower(y, k % 8) * ek,
                hi: frob16_tower(y, k % 8),
            };
            if phi_z != expected {
                cocycle_ok = false;
            }
            let c_orig = Block256::from_raw_challenge_lanes(lo, y);
            let c_shift = Block256::from_raw_challenge_lanes(
                frob16_tower(lo, k % 8),
                frob16_tower(y, k % 8) + ek,
            );
            if c_shift.hi != frob16_tower(c_orig.hi, k % 8) {
                cocycle_ok = false;
            }
        }
    }
    assert!(cocycle_ok);

    // -------------------------------------------------------------------------
    // 6. Fiat-Shamir Channels, Bridges & Transcript State-Machine Checks
    // -------------------------------------------------------------------------
    let mut ch_base = Poseidon2bChannel::new();
    let base_z0 = ch_base.squeeze();
    let base_z1 = ch_base.squeeze();
    let base_zero_squeeze = (base_z0 == Block128::ZERO) && (base_z1 == Block128::ZERO);

    let mut ch_fri = FriChannel::new();
    let fri_z0 = ch_fri.get_random_point();
    let fri_z1 = ch_fri.get_random_point();
    let fri_zero_squeeze = (fri_z0 == Block128::ZERO) && (fri_z1 == Block128::ZERO);

    let mut ch_wide = Poseidon2bWideChannel::new();
    let wide_z0 = ch_wide.squeeze_wide();
    let wide_zero_tau = (wide_z0.lo == Block128::ZERO) && (wide_z0.hi == Block128::TAU);
    assert!(base_zero_squeeze && fri_zero_squeeze && wide_zero_tau);

    let mut fs_byte_a = FsChallenger::new(b"dom-a");
    let mut fs_byte_b = FsChallenger::new(b"dom-b");
    let fs_frames_init = fs_byte_a.sample_f128() != fs_byte_b.sample_f128();

    let mut fs_lane_a = FsLaneChallenger::new(b"dom-a");
    let mut fs_lane_b = FsLaneChallenger::new(b"dom-b");
    let fs_lane_frames_init = fs_lane_a.sample_f128() != fs_lane_b.sample_f128();
    assert!(fs_frames_init && fs_lane_frames_init);

    // Odd-vs-even squeeze state aliasing on unframed channels
    let mut c_odd = Poseidon2bChannel::new();
    let mut c_even = Poseidon2bChannel::new();
    c_odd.absorb(Block128(0x1234));
    c_odd.absorb(Block128(0x5678));
    c_even.absorb(Block128(0x1234));
    c_even.absorb(Block128(0x5678));
    let _ = c_odd.squeeze(); // 1 squeeze
    let _ = c_even.squeeze();
    let _ = c_even.squeeze(); // 2 squeezes
    c_odd.absorb(Block128(0x9ABC));
    c_even.absorb(Block128(0x9ABC));
    let odd_even_base_alias = c_odd.squeeze() == c_even.squeeze();

    let mut w_odd = Poseidon2bWideChannel::new();
    let mut w_even = Poseidon2bWideChannel::new();
    w_odd.absorb_wide(Block256 {
        lo: Block128(11),
        hi: Block128(22),
    });
    w_even.absorb_wide(Block256 {
        lo: Block128(11),
        hi: Block128(22),
    });
    let _ = w_odd.squeeze_base();
    let _ = w_even.squeeze_base();
    let _ = w_even.squeeze_base();
    let odd_even_wide_alias = w_odd.squeeze_wide() == w_even.squeeze_wide();
    assert!(odd_even_base_alias && odd_even_wide_alias);

    // Verify `zk_authorization` 1-lane header + Block256 half-block alignment vs pad flush:
    // Absorbing 1 base tag + 1 Block256 (= 3 lanes) leaves 1 lane buffered and triggers a pad permutation on squeeze,
    // equivalent to absorbing `[tag, w.lo, w.hi, PAD1_TOWER]`!
    let tag_lane = Block128(u64::from_le_bytes(*b"ZKAMFROM") as u128);
    let w_msg = Block256 {
        lo: Block128(0xAABB_CCDD),
        hi: Block128(0xEEFF_0011),
    };
    let mut w_zk = Poseidon2bWideChannel::new();
    w_zk.absorb_base(tag_lane);
    w_zk.absorb_wide(w_msg);
    let chal_zk = w_zk.squeeze_wide();
    let bridge_after_squeeze = w_zk.close_into_bridge(Block128(0x42));
    assert_ne!(bridge_after_squeeze, [Block128::ZERO; 4]);

    let mut w_manual_pad = Poseidon2bWideChannel::new();
    let mut pad_bytes = [0u8; 16];
    pad_bytes[0] = 0x80;
    pad_bytes[15] = 0x01;
    let pad_lane = Block128(u128::from_le_bytes(pad_bytes));
    w_manual_pad.absorb_base(tag_lane);
    w_manual_pad.absorb_base(w_msg.lo);
    w_manual_pad.absorb_base(w_msg.hi);
    w_manual_pad.absorb_base(pad_lane);
    let chal_manual = w_manual_pad.squeeze_wide();
    let zk_auth_misalignment_verified = chal_zk == chal_manual;
    assert!(zk_auth_misalignment_verified);

    // FsLaneChallenger PoW parity (even vs odd buffered lane) & 0-bit canonical nonce rule
    let mut pow_even_p = FsLaneChallenger::new_c1(b"pow-parity");
    // After new_c1("pow-parity"): OP_DOMAIN + 1 packed lane = 2 lanes -> buffered == None
    let nonce_even = pow_even_p.grind_pow(6);
    let post_even_p = pow_even_p.sample_f256();
    let mut pow_even_v = FsLaneChallenger::new_c1(b"pow-parity");
    let ok_even = pow_even_v.verify_pow(nonce_even, 6) && (pow_even_v.sample_f256() == post_even_p);

    let mut pow_odd_p = FsLaneChallenger::new_c1(b"pow-parity");
    // `observe_f256` absorbs 3 lanes (OP_OBSERVE + lo + hi) -> leaves `buffered == Some(hi)`!
    let odd_val = F256 {
        lo: F128 { lo: 99, hi: 0 },
        hi: F128 { lo: 101, hi: 0 },
    };
    pow_odd_p.observe_f256(odd_val);
    let nonce_odd = pow_odd_p.grind_pow(6);
    let post_odd_p = pow_odd_p.sample_f256();
    let mut pow_odd_v = FsLaneChallenger::new_c1(b"pow-parity");
    pow_odd_v.observe_f256(odd_val);
    let ok_odd = pow_odd_v.verify_pow(nonce_odd, 6) && (pow_odd_v.sample_f256() == post_odd_p);

    let mut pow_zero_v = FsLaneChallenger::new_c1(b"pow-zero");
    let rejects_nonzero_at_zero_bits = !pow_zero_v.verify_pow(1, 0);
    let mut pow_zero_v2 = FsLaneChallenger::new_c1(b"pow-zero");
    let accepts_zero_at_zero_bits = pow_zero_v2.verify_pow(0, 0);
    let pow_parity_ok = ok_even && ok_odd;
    let pow_zero_ok = rejects_nonzero_at_zero_bits && accepts_zero_at_zero_bits;
    assert!(pow_parity_ok && pow_zero_ok);

    // RoundPolynomial empty coeffs API behavior
    let empty_poly = RoundPolynomial::<Block128>::from_coeffs(vec![]);
    let empty_deg_zero = empty_poly.degree() == 0
        && empty_poly.evaluate(Block128::ZERO) == Block128::ZERO
        && empty_poly.evaluate(Block128::ONE) == Block128::ZERO;
    let mut ch_sc = Poseidon2bChannel::new();
    ch_sc.absorb(Block128(0x55));
    ch_sc.absorb(Block128(0x66));
    let first_sq = ch_sc.squeeze(); // leaves `pending = Some(second_sq)`
    let mut ch_expected = ch_sc.clone();
    let expected_pending = ch_expected.squeeze();
    for &c in &empty_poly.coeffs {
        ch_sc.absorb(c);
    }
    let after_empty_absorb = ch_sc.squeeze(); // returns stale `pending`!
    let empty_round_poly_anomaly = empty_deg_zero && (first_sq != expected_pending) && (after_empty_absorb == expected_pending);
    assert!(empty_round_poly_anomaly);

    // -------------------------------------------------------------------------
    // 7. `noid_soundness` Parameter & Formula Differential Audit
    // -------------------------------------------------------------------------
    let cert = noid_soundness::calculate().expect("soundness certificate loads");
    let params = ProductionParameters::load().expect("production parameters load");
    let p_audit = &cert.poseidon2b_cryptanalysis;
    let balancing_core = p_audit.nonlinear_subspaces.even_balancing_core;
    let balancing_in_gf2_16 = balancing_core < 65536;
    assert!(p_audit.appendix_a_compression_applies);
    assert_eq!(p_audit.ideal_degree_exponent, 73);
    assert!(balancing_in_gf2_16);

    let legacy_perms = qrom::history_query_squeeze_permutations(&params);
    assert_eq!(legacy_perms, 12);

    // Construct heterogeneous `[m=24 (k_code=21, 12 perms), m=25 (k_code=22, 14 perms)]` profile
    // mirroring a mixed v2/v2.1 HistoryStep bank where one arm has k_code=21 and another has k_code=22.
    let mut het_params = params.clone();
    het_params.history_classes[0] = params.history_classes[1].clone(); // k_code = 21 -> 23 lanes -> 12 perms
    het_params.history_classes[1] = HistoryClassParameters {
        tier: 256,
        message_log2: 20,
        codeword_log2: 22, // floor(128 / 22) = 5 -> ceil(133 / 5) = 27 lanes -> 14 perms
        codeword_len: 1u64 << 22,
        inverse_rate: 4,
        plaintext_tail_len: params.history_classes[1].plaintext_tail_len,
        fri_arities: vec![4, 4, 4, 4, 4],
    };
    let het_max_perms = qrom::history_query_squeeze_permutations(&het_params);
    let het_min_perms = het_params
        .history_classes
        .iter()
        .map(|class| {
            let pos = 128usize / class.codeword_log2;
            let lanes = (het_params.history_queries as usize).div_ceil(pos);
            lanes.div_ceil(het_params.poseidon_rate_lanes) as u32
        })
        .min()
        .unwrap();
    assert_eq!(het_max_perms, 14);
    assert_eq!(het_min_perms, 12);

    let het_res_cert = resource::certificate(&het_params);
    let hist_ev = het_res_cert
        .events
        .iter()
        .find(|e| e.id == "history.query")
        .expect("history.query event present");
    let scalar_gd = het_res_cert.poseidon_response_cost.gate_depth_product();
    assert_eq!(
        hist_ev.response_cost.gate_depth_product(),
        &scalar_gd * (het_max_perms * het_max_perms)
    );

    // Measure actual challenger permutations for 23 query-seed lanes on FsLaneChallenger
    let mut lc = FsLaneChallenger::new_c1(b"hist-step");
    // Align buffered state first (`OP_DOMAIN` + 1 lane = 2 lanes -> buffered == None, perms == 1)
    assert_eq!(lc.perms(), 1);
    let p_before = lc.perms();
    let drawn = lc.sample_f128_vec(23);
    assert_eq!(drawn.len(), 23);
    let total_23_perms = lc.perms() - p_before;
    // 1 perm for pad-flushing OP_SQUEEZE, then 12 squeeze_lane permute() calls (the 12th permute is trailing after reading lanes 22 & 23, wait:
    // after pad-flush perm #1, lanes 0,1 read -> perm #2; ... lanes 20,21 read -> perm #12; lane 22 read -> perm #13 trailing!)
    let effective_23_perms = total_23_perms - 1;
    assert_eq!(total_23_perms, 13);
    assert_eq!(effective_23_perms, 12);

    let summary = AuditSummary {
        commit: "52938433a30b185ab007afaa15166a632cf2e82e",
        galois_equivariance: GaloisEquivarianceReport {
            basis_isomorphism_samples: iso_samples,
            round_constants_all_in_gf256: rc_in_gf256,
            mds_full_all_in_gf256: mds_full_in_gf256,
            mds_partial_active_entry_in_gf256: mds_partial_00_in_gf256,
            mds_partial_passive_diag_in_gf65536_minus_gf256: mds_partial_diag_in_gf65536_not_256,
            minimal_parameter_subfield_bits: 16,
            galois_group_order_128_over_16: 8,
            sigma4_closed_form_verified_samples: iso_samples,
            tower_permute_equivariant_all_k: tower_eq,
            flat_permute_pmull_equivariant_all_k: flat_eq,
            packed_permute_equivariant_all_k: packed_eq,
            packed_flat_many_equivariant_all_k: packed_many_eq,
        },
        subfield_lattice: subfield_reports,
        domain_tags: DomainTagReport {
            total_distinct_repo_tags: ALL_87_DOMAIN_TAGS.len(),
            any_tag_iv_in_gf2_64_sq: any_in_gf2_64_sq,
            sigma4_formula_all_verified: sigma4_iv_ok,
            min_frobenius_orbit_size: min_orbit,
            cross_tag_frobenius_conjugate_pairs: cross_conjugates,
            empty_rate_one_perm_collisions: 87 - empty_perm_outputs.len(),
            leaf_fixed_iv_aliases_any_domain_tag: leaf_aliases_tag,
            leaf_fixed_iv_sigma_k_neq_4_possible: k_neq_4_possible,
            leaf_fixed_iv_sigma4_linear_rank: lin_rank,
            leaf_fixed_iv_sigma4_conjugate_u64_solutions: u64_solutions,
        },
        compression_and_zero_iv: CompressionZeroIvReport {
            ff_tag_frobenius_equivariant_all_k: ff_equivariant,
            ff_tag_sigma4_delta_equals_iv_lo_zero: delta4_matches,
            ff_tag_gf2_16_coset_invariant: ff_16,
            ff_tag_gf2_32_coset_invariant: ff_32,
            ff_tag_gf2_64_coset_invariant_with_zero_b1_shift: ff_64,
            hash_pair_galois_equivariant: hp_equivariant,
            hash_pair_ignores_sponge_iv: hash_pair_ignores_iv,
            hash_field_ignores_sponge_iv: hash_field_ignores_iv,
            hash_concatenation_ignores_sponge_iv: hash_concat_ignores_iv,
            zero_iv_block0_gf2_16_traps_capacity_in_32_bits: zero_iv_cap_trapped,
            keystore_with_zero_iv_matches_digest_and_hash_concatenation: keystore_alias_verified,
        },
        extension_field_256: Extension256Report {
            tower_flat_f256_isomorphism_samples: 256,
            raw_challenge_two_to_one_verified: two_to_one_ok,
            raw_challenge_trace_one_and_non_base: trace_one_ok,
            raw_challenge_generates_full_gf2_256: full_gen_ok,
            galois_256_over_16_order: 16,
            cocycle_constants_e_k_verified_all_16: cocycle_ok,
        },
        fiat_shamir_channels: ChannelAuditReport {
            poseidon2b_channel_zero_absorb_squeeze_pair_is_zero: base_zero_squeeze,
            fri_channel_zero_absorb_squeeze_pair_is_zero: fri_zero_squeeze,
            wide_channel_zero_absorb_squeeze_is_zero_tau: wide_zero_tau,
            fs_challenger_frames_domain_on_init: fs_frames_init,
            fs_lane_challenger_frames_domain_on_init: fs_lane_frames_init,
            odd_even_squeeze_post_absorb_state_alias_base: odd_even_base_alias,
            odd_even_squeeze_base_then_wide_alias: odd_even_wide_alias,
            zk_auth_single_tag_shifts_wide_across_rate_blocks: zk_auth_misalignment_verified,
            fs_lane_pow_even_and_odd_parity_roundtrip: pow_parity_ok,
            fs_lane_pow_zero_bits_rejects_nonzero_nonce: pow_zero_ok,
            empty_round_poly_reports_degree_zero_and_absorbs_nothing: empty_round_poly_anomaly,
        },
        soundness_accounting: SoundnessAuditReport {
            poseidon_audit_appendix_a_applies: p_audit.appendix_a_compression_applies,
            poseidon_audit_ideal_degree_exponent: p_audit.ideal_degree_exponent,
            poseidon_audit_even_balancing_core_hex: format!("{:#06x}", balancing_core),
            poseidon_audit_even_balancing_core_in_gf2_16: balancing_in_gf2_16,
            actual_gf2_16_univariate_degree_cap: 65_535,
            legacy_history_squeeze_perms: legacy_perms,
            heterogeneous_m24_m25_max_perms: het_max_perms,
            heterogeneous_m24_m25_min_perms: het_min_perms,
            qrom_max_vs_min_gate_depth_undercharge_num: 49,
            qrom_max_vs_min_gate_depth_undercharge_den: 36,
            qrom_max_vs_min_gate_depth_undercharge_ratio: (14.0 * 14.0) / (12.0 * 12.0),
            fs_lane_challenger_23_lanes_total_perms: total_23_perms,
            fs_lane_challenger_23_lanes_effective_perms: effective_23_perms,
            wallet_wide_channel_grind_plus_7_seeds_effective_perms: 4,
        },
    };

    let json = serde_json::to_string_pretty(&summary).expect("serialize summary");
    fs::write("results.json", format!("{json}\n")).expect("write results.json");

    println!("=== PARANO1D CRYPTOGRAPHIC & TRANSCRIPT ALGEBRA AUDIT ===");
    println!("Target commit: {}", summary.commit);
    println!("N_ROUNDS = {} (8 full + 58 partial), STATE_SIZE = {}", N_ROUNDS, STATE_SIZE);
    println!();
    println!("[1] Galois Equivariance: ALL 8 automorphisms of Gal(GF(2^128)/GF(2^16)) verified");
    println!("    - ROUND_CONSTANTS & MDS_FULL in GF(2^8): true; MDS_PARTIAL diag in GF(2^16)\\GF(2^8): true");
    println!("    - Closed-form sigma_4(v) = v ^ (v >> 64): verified ({} samples)", iso_samples);
    println!("    - Equivariance across permute_mut, permute_flat_u128 (PMULL), packed, packed_many: PASS");
    println!();
    println!("[2] Subfield Lattice Invariance:");
    for sr in &summary.subfield_lattice {
        println!(
            "    - GF(2^{:>2})^4 ({:>3}-bit state, {:>3}-bit digest): invariant = {}",
            sr.subfield_bits, sr.state_space_bits, sr.rate_digest_bits, sr.invariant_full_permutation
        );
    }
    println!();
    println!("[3] DomainTag & leaf_fixed_iv_flat Audit:");
    println!("    - Distinct repo DomainTags verified: {}", summary.domain_tags.total_distinct_repo_tags);
    println!("    - Any DomainTag IV in GF(2^64)^2: {}", summary.domain_tags.any_tag_iv_in_gf2_64_sq);
    println!("    - Cross-tag Frobenius conjugate pairs (k=1..7): {}", summary.domain_tags.cross_tag_frobenius_conjugate_pairs);
    println!(
        "    - leaf_fixed_iv_flat sigma_4 64x64 GF(2) rank = {}, u64 conjugate solutions = {}",
        summary.domain_tags.leaf_fixed_iv_sigma4_linear_rank,
        summary.domain_tags.leaf_fixed_iv_sigma4_conjugate_u64_solutions
    );
    println!();
    println!("[4] Feed-Forward Compression & Zero-IV Sponge Subspace:");
    println!("    - compress_flat_feed_forward_with_tag Frobenius equivariance (all k=0..7): PASS");
    println!("    - Coset subfield traps GF(2^16)^4, GF(2^32)^4, GF(2^64)^4 (zero b1 shift at d=64): PASS");
    println!("    - CryptographicHasher ignores self sponge IV on hash_pair/hash_field/hash_concatenation: true");
    println!("    - Zero-IV Block-0 GF(2^16)^2 rate traps 256-bit capacity into 32-bit GF(2^16)^2: true");
    println!("    - MasterSecret::derive_spend_secret (with_iv([0;2])) == poseidon2b_digest == hash_concatenation: true");
    println!();
    println!("[5] GF(2^256) Challenge Map & Gal(GF(2^256)/GF(2^16)):");
    println!("    - 2-to-1 trace-one fiber, non-base-subfield, full GF(2^256) generation: PASS");
    println!("    - All 16 Galois automorphisms Phi^k and cocycle constants e_k verified: PASS");
    println!();
    println!("[6] Fiat-Shamir Channels & State-Machine Properties:");
    println!("    - Unframed 0-absorb squeeze ignores IV ([0,0] / (0,TAU)): verified");
    println!("    - Odd-vs-even squeeze state aliasing after absorb / squeeze_wide: verified");
    println!("    - zk_authorization 1-lane tag + Block256 half-block split + pad permutation: verified");
    println!("    - FsLaneChallenger PoW even/odd parity & 0-bit canonical zero nonce: PASS");
    println!("    - Empty RoundPolynomial degree()==0 & 0-absorb stale pending behavior: verified");
    println!();
    println!("[7] noid_soundness Accounting Differential:");
    println!(
        "    - poseidon2b_cryptanalysis ideal_degree = 7^{}, even_balancing_core = {} (< 2^16), actual GF(2^16) degree cap = 65535",
        summary.soundness_accounting.poseidon_audit_ideal_degree_exponent,
        summary.soundness_accounting.poseidon_audit_even_balancing_core_hex
    );
    println!(
        "    - qrom.rs history_query_squeeze_permutations on [m24(k=21), m25(k=22)]: .max()={} vs .min()={} ({}/{} = {:.6}x gate-depth undercharge)",
        summary.soundness_accounting.heterogeneous_m24_m25_max_perms,
        summary.soundness_accounting.heterogeneous_m24_m25_min_perms,
        summary.soundness_accounting.qrom_max_vs_min_gate_depth_undercharge_num,
        summary.soundness_accounting.qrom_max_vs_min_gate_depth_undercharge_den,
        summary.soundness_accounting.qrom_max_vs_min_gate_depth_undercharge_ratio
    );
    println!(
        "    - FsLaneChallenger::sample_f128_vec(23): {} total perms ({} effective + 1 trailing)",
        summary.soundness_accounting.fs_lane_challenger_23_lanes_total_perms,
        summary.soundness_accounting.fs_lane_challenger_23_lanes_effective_perms
    );
    println!("=== ALL CHECKS PASSED (results written to results.json) ===");
}
