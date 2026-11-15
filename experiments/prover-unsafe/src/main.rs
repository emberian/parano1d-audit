use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use noid_ivc_core::challenger::Challenger;
use noid_ivc_core::field::F128;
use noid_ivc_core::ntt::{AdditiveNttF128, AdditiveNttGf8, InvNttTableByteSingleGf8};
use noid_ivc_core::pcs::basefold;
use noid_ivc_core::{merkle, scratch};

struct CountingAllocator;
static MAX_REQUEST: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        MAX_REQUEST.fetch_max(layout.size(), Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        MAX_REQUEST.fetch_max(new_size, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[derive(Default)]
struct TestChallenger(u64);
impl Challenger for TestChallenger {
    fn observe_f128(&mut self, _value: F128) {}

    fn sample_f128(&mut self) -> F128 {
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        let lo = z ^ (z >> 31);
        F128::new(lo, lo.rotate_left(29))
    }
}

fn f128_bytes(values: &[F128]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| {
            value
                .lo
                .to_le_bytes()
                .into_iter()
                .chain(value.hi.to_le_bytes())
        })
        .collect()
}

fn uninit_zero_round() {
    scratch::clear();
    for index in 0..5u64 {
        let mut poisoned = scratch::take_f128(1);
        poisoned[0] = F128::new(0xD15C_0000_0000_0000 + index, 0x1EAD_ED00_0000_0000 + index);
        scratch::give_f128(poisoned);
    }

    let witness = [F128::new(0x11, 0x22)];
    let tensor = vec![F128::new(0x33, 0x44)];
    let codeword = [F128::new(0x55, 0x66), F128::new(0x77, 0x88)];
    let tree = merkle::merkle_tree_sequential(&f128_bytes(&codeword), 2);
    let ntt = AdditiveNttF128::standard(1);
    let proof = basefold::prove_with_precomputed_round0_prime(
        &witness,
        tensor,
        F128::ZERO,
        &codeword,
        &tree,
        &ntt,
        1,
        0,
        1,
        Some((F128::ZERO, F128::ZERO)),
        &mut TestChallenger::default(),
    );

    println!(
        "zero-round final_a={:016x}:{:016x}",
        proof.final_a.hi, proof.final_a.lo
    );
    for (index, value) in proof.final_codeword.iter().enumerate() {
        println!(
            "zero-round final_codeword[{index}]={:016x}:{:016x}",
            value.hi, value.lo
        );
    }
    assert_eq!(
        proof.final_a,
        F128::new(0xD15C_0000_0000_0004, 0x1EAD_ED00_0000_0004)
    );
    assert_ne!(proof.final_a, witness[0]);
    scratch::clear();
}

fn parser_allocation(label: &str, declared: u64) {
    let mut bytes = b"NOIDI".to_vec();
    bytes.extend_from_slice(&[0u8; 60]);
    bytes.extend_from_slice(&declared.to_le_bytes());
    MAX_REQUEST.store(0, Ordering::Relaxed);
    let result = noid_ivc_prover::proof_io::R1csProofBundle::from_bytes(&bytes);
    println!(
        "parser {label}: input={} declared={declared} max_single_allocation={} result={}",
        bytes.len(),
        MAX_REQUEST.load(Ordering::Relaxed),
        match result {
            Ok(_) => "Ok".to_string(),
            Err(error) => error.to_string(),
        }
    );
}

fn parser() {
    parser_allocation("small", 64);
    parser_allocation("hostile", 10_000_000);
}

#[derive(Clone)]
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }

    fn f128(&mut self) -> F128 {
        F128::new(self.next(), self.next())
    }
}

fn small_inputs() {
    let result = std::panic::catch_unwind(|| AdditiveNttF128::standard(0));
    match result {
        Err(_) => println!("small input: AdditiveNttF128::standard(0) panicked at empty eval row"),
        Ok(_) => panic!("zero-dimensional NTT unexpectedly constructed"),
    }
    let result =
        std::panic::catch_unwind(|| noid_ivc_core::pcs::basefold::default_fri_queries(0, 1));
    match result {
        Err(_) => println!(
            "small input: basefold::default_fri_queries(0, 1) panicked (finite-length precondition -> unwrap_or_else(panic))"
        ),
        Ok(_) => panic!("degenerate FRI query count unexpectedly derived"),
    }
}

fn differential() {
    let mut rng = Rng(0x52938433);
    for log_d in 1..=10 {
        let ntt = AdditiveNttF128::standard(log_d);
        let input: Vec<F128> = (0..1usize << log_d).map(|_| rng.f128()).collect();
        let mut scalar = input.clone();
        let mut dispatched = input.clone();
        ntt.forward_transform_scalar(&mut scalar);
        ntt.forward_transform(&mut dispatched);
        assert_eq!(scalar, dispatched, "NTT dispatch mismatch at log_d={log_d}");
        ntt.inverse_transform(&mut dispatched);
        assert_eq!(input, dispatched, "NTT roundtrip mismatch at log_d={log_d}");

        for lanes in [1, 2, 4, 8] {
            let input: Vec<F128> = (0..(1usize << log_d) * lanes).map(|_| rng.f128()).collect();
            let mut scalar = input.clone();
            let mut dispatched = input.clone();
            ntt.forward_transform_interleaved_scalar(&mut scalar, lanes);
            ntt.forward_transform_interleaved(&mut dispatched, lanes);
            assert_eq!(
                scalar, dispatched,
                "interleaved NTT mismatch log_d={log_d} lanes={lanes}"
            );
        }
    }

    for k in 3..=7 {
        let ntt_s = AdditiveNttGf8::new(k, noid_ivc_core::field::F8::ZERO);
        let ntt_l = AdditiveNttGf8::new(k, noid_ivc_core::field::F8(1 << k));
        let table = InvNttTableByteSingleGf8::new(&ntt_s, &ntt_l);
        for _ in 0..128 {
            let bytes: Vec<u8> = (0..table.n_chunks).map(|_| rng.next() as u8).collect();
            let mut scalar = vec![noid_ivc_core::field::F8::ZERO; table.ell];
            let mut dispatched = scalar.clone();
            table.apply_scalar(&bytes, &mut scalar);
            table.apply(&bytes, &mut dispatched);
            assert_eq!(scalar, dispatched, "inverse table mismatch at k={k}");
        }
    }

    for leaves in [1usize, 2, 3, 4, 5, 8, 16, 17] {
        let leaf_size = 64;
        let data: Vec<u8> = (0..leaves * leaf_size).map(|_| rng.next() as u8).collect();
        let iv = [
            rng.next() as u128 | ((rng.next() as u128) << 64),
            rng.next() as u128,
        ];
        let mut batch = vec![[0u8; 32]; leaves];
        noid_poseidon2b::batch::leaf_sponge_flat_batch_with_iv_into(
            iv, false, &data, leaf_size, &mut batch,
        );
        for (leaf, actual) in data.chunks_exact(leaf_size).zip(batch) {
            let mut sponge = noid_poseidon2b::native::Poseidon2bFlatSponge::with_iv_flat(iv);
            sponge.update(leaf);
            assert_eq!(
                sponge.finalize_no_pad(),
                actual,
                "Poseidon batch mismatch leaves={leaves}"
            );
        }
    }
    println!("differential: NTT, inverse-table, and Poseidon native/batch checks passed");
}

fn mul_kernels() {
    use noid_ivc_core::field::gf2_128::software;
    use noid_ivc_core::field::{F8, F256};

    let mut rng = Rng(0xC0FFEE);
    let edges = [
        F128::ZERO,
        F128::ONE,
        F128::generator(),
        F128::new(u64::MAX, u64::MAX),
        F128::new(0, u64::MAX),
        F128::new(u64::MAX, 0),
        F128::new(1 << 63, 1 << 63),
    ];
    let mut pairs: Vec<(F128, F128)> = Vec::new();
    for &a in &edges {
        for &b in &edges {
            pairs.push((a, b));
        }
    }
    for _ in 0..4096 {
        pairs.push((rng.f128(), rng.f128()));
        let a = rng.f128();
        pairs.push((a, a));
    }

    let mut cases = 0usize;
    for &(a, b) in &pairs {
        let reference = software::ghash_mul(a, b);
        // The production dispatch (NEON PMULL on this aarch64 build).
        assert_eq!(
            a * b,
            reference,
            "F128::mul dispatch != software at a={a:?}"
        );
        assert_eq!(
            a.mul_unreduced(b).reduce(),
            reference,
            "unreduced+reduce != software at a={a:?}"
        );
        assert_eq!(software::ghash_mul_unreduced(a, b).reduce(), reference);
        #[cfg(target_arch = "aarch64")]
        {
            use noid_ivc_core::field::gf2_128::aarch64;
            assert_eq!(unsafe { aarch64::ghash_mul_binius(a, b) }, reference);
            assert_eq!(unsafe { aarch64::ghash_mul_schoolbook(a, b) }, reference);
            assert_eq!(unsafe { aarch64::ghash_mul_karatsuba(a, b) }, reference);
            assert_eq!(
                unsafe { aarch64::ghash_mul_karatsuba_barrett(a, b) },
                reference
            );
            assert_eq!(
                unsafe { aarch64::ghash_mul_unreduced_neon(a, b).reduce() },
                reference
            );
            // Two-lane batch kernel; second lane proves lane independence.
            let lane_b = F128::new(0x5A5A_5A5A_5A5A_5A5A, 0xA5A5_A5A5_A5A5_A5A5);
            let pair = unsafe { aarch64::ghash_mul_vec2_neon([a, F128::ZERO], [b, lane_b]) };
            assert_eq!(pair[0], reference);
            assert_eq!(pair[1], F128::ZERO);
        }
        cases += 1;
    }

    // C1 lift consistency: base-field product must commute with the lift.
    for _ in 0..1024 {
        let a = rng.f128();
        let b = rng.f128();
        assert_eq!(
            F256::from_base(a) * F256::from_base(b),
            F256::from_base(a * b),
            "F256 lift inconsistency at a={a:?}"
        );
    }

    // GF(2^8) 16-lane SIMD kernel vs scalar multiply.
    #[cfg(target_arch = "aarch64")]
    {
        for _ in 0..1024 {
            let av: [u8; 16] = std::array::from_fn(|_| rng.next() as u8);
            let bv: [u8; 16] = std::array::from_fn(|_| rng.next() as u8);
            let a_vec = unsafe { std::arch::aarch64::vld1q_u8(av.as_ptr()) };
            let b_vec = unsafe { std::arch::aarch64::vld1q_u8(bv.as_ptr()) };
            let result = unsafe { noid_ivc_core::field::gf2_8::neon::gf8_mul_vec16(a_vec, b_vec) };
            let bytes: [u8; 16] = unsafe { std::mem::transmute(result) };
            for i in 0..16 {
                assert_eq!(F8(bytes[i]), F8(av[i]) * F8(bv[i]), "gf8 lane {i}");
            }
        }
    }

    println!(
        "mul kernels: {cases} F128 pairs agree across dispatch/unreduced-reduce/schoolbook/karatsuba/barrett/binius/vec2; F256 lift consistent; F8 vec16 == scalar"
    );
}

fn c1_uninit_zero_round() {
    use noid_ivc_core::field::F256;

    // Recycle-poison the 32-byte malloc size class the C1 prover draws its
    // length-1 F256 ping-pong buffers from (`alloc_uninit_vec`, not the pool).
    let poison = F256 {
        lo: F128::new(0xD15C_C1A0_0000_0001, 0x1EAD_EDC1_0000_0001),
        hi: F128::new(0xBA1D_BA1D_0000_0000, 0x0DDB_0DDB_0000_0000),
    };
    for _ in 0..128 {
        let mut bait: Vec<F256> = Vec::with_capacity(1);
        bait.push(poison);
        drop(bait);
    }

    let witness = [F128::new(0x11, 0x22)];
    let tensor = vec![F256::from_base(F128::new(0x33, 0x44))];
    let target = F256::from_base(witness[0]) * tensor[0];
    // log_inv_rate = 5 keeps checked_fri_configuration's finite-length
    // precondition satisfied at log_dim = 0, so the production query count is
    // derivable and prove/verify both run end to end.
    let log_inv_rate = 5usize;
    let n_queries = noid_ivc_core::pcs::basefold::checked_fri_configuration(0, log_inv_rate)
        .unwrap()
        .query_count;
    let codeword = vec![witness[0]; 1usize << log_inv_rate];
    let tree = merkle::merkle_tree_sequential(&f128_bytes(&codeword), codeword.len());
    let root = *tree.last().expect("non-empty tree");
    let ntt = AdditiveNttF128::standard(log_inv_rate);
    let proof = basefold::prove_c1_with_precomputed_round0_prime(
        &witness,
        tensor.clone(),
        target,
        &codeword,
        &tree,
        &ntt,
        log_inv_rate,
        0,
        n_queries,
        Some((F256::ZERO, F256::ZERO)),
        &mut TestChallenger::default(),
    );

    println!(
        "c1 zero-round final_a=({:016x}:{:016x},{:016x}:{:016x}) expected_honest=({:016x}:{:016x})",
        proof.final_a.hi.hi,
        proof.final_a.hi.lo,
        proof.final_a.lo.hi,
        proof.final_a.lo.lo,
        witness[0].hi,
        witness[0].lo,
    );
    let marker_hit = proof.final_a == poison;
    println!("c1 zero-round malloc-recycle marker observed: {marker_hit}");
    assert_ne!(
        proof.final_a,
        F256::from_base(witness[0]),
        "zero-round C1 final_a must not be the honest value (it is never written)"
    );

    // The garbage-bearing proof must fail verification (scope: prover-side
    // correctness/UB, not verifier acceptance).
    let verdict = basefold::verify_c1(
        target,
        &proof,
        &root,
        &ntt,
        0,
        log_inv_rate,
        0,
        &mut TestChallenger::default(),
    );
    println!("c1 zero-round verify verdict: {ver:?}", ver = &verdict);
    assert!(matches!(
        verdict,
        Err(noid_ivc_core::pcs::basefold::VerifyError::SumcheckFinalMismatch)
    ));
    scratch::clear();
}

fn matrix_fuzz() {
    use noid_ivc_core::field_r1cs::{FieldR1cs, SparseFieldMatrix};
    use noid_ivc_core::proof::FieldShape;

    let shape = FieldShape {
        m: 7,
        k_log: 7,
        k_skip: 6,
        const_pin: None,
    };
    let relation = FieldR1cs {
        m: shape.m,
        k_log: shape.k_log,
        k_skip: shape.k_skip,
        useful_rows: 128,
        a_0: SparseFieldMatrix::identity(128),
        b_0: SparseFieldMatrix::zero(128),
        const_pin: None,
        digest_cache: std::sync::OnceLock::new(),
        csc_cache: std::sync::OnceLock::new(),
    };
    let mut canonical = Vec::new();
    relation.write_artifact(&mut canonical).unwrap();
    let digest = relation.structural_statement_digest();
    let loaded = FieldR1cs::read_artifact(
        &mut std::io::Cursor::new(&canonical),
        shape,
        digest,
        canonical.len(),
    );
    assert!(loaded.is_ok());

    let mut rng = Rng(0xF022);
    let mut cases = 0usize;
    for cut in 0..=canonical.len() {
        let input = canonical[..cut].to_vec();
        let result = std::panic::catch_unwind(|| {
            FieldR1cs::read_artifact_unbound(
                &mut std::io::Cursor::new(input),
                shape,
                canonical.len(),
            )
        });
        assert!(result.is_ok(), "panic on truncated matrix at {cut}");
        cases += 1;
    }
    for _ in 0..2000 {
        let mut input = canonical.clone();
        for _ in 0..1 + rng.next() % 8 {
            let at = (rng.next() as usize) % input.len();
            input[at] ^= 1u8 << (rng.next() % 8);
        }
        if rng.next() & 1 != 0 {
            let new_len = (rng.next() as usize) % (canonical.len() + 32);
            input.resize(new_len, rng.next() as u8);
        }
        let max = canonical.len() + 32;
        let result = std::panic::catch_unwind(|| {
            FieldR1cs::read_artifact_unbound(&mut std::io::Cursor::new(input), shape, max)
        });
        assert!(result.is_ok(), "panic on mutated matrix case {cases}");
        cases += 1;
    }
    println!("matrix loader: {cases} truncation/mutation cases returned without panic");
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "all".to_string());
    match mode.as_str() {
        "uninit" => uninit_zero_round(),
        "c1-uninit" => c1_uninit_zero_round(),
        "parser" => parser(),
        "small-inputs" => small_inputs(),
        "differential" => differential(),
        "mul-kernels" => mul_kernels(),
        "matrix-fuzz" => matrix_fuzz(),
        "all" => {
            uninit_zero_round();
            c1_uninit_zero_round();
            parser();
            small_inputs();
            differential();
            mul_kernels();
            matrix_fuzz();
        }
        _ => panic!("unknown mode {mode}"),
    }
}
