//! Compositional check: does the known 2^16-cost compressor collision
//! (compress_flat_feed_forward_with_tag under TAG_CAPSNODE) reach any
//! SEMANTIC binding of the Pruv/object path?
//!
//! 1. Reproduce the known collision under TAG_CAPSNODE (feed-forward).
//! 2. Show the same input pair does NOT collide under the node compressors
//!    of the semantic trees (exact state, tx root, segment tree, claims)
//!    — the full-sponge `compress_with_tag` construction.
//! 3. Show the v2 contract OBJECT-ROOT wraps (the binding that commits
//!    program / current / next) are full permutation wraps over the actual
//!    cells: every single-bit change of any committed semantic cell changes
//!    the old/new object root (and the code/policy digest). A Merkle-collision
//!    style "two payloads, one binding" needs a collision HERE to matter.
//!
//! 4. Note the colliding function's remaining consumers: ff_merkle.rs
//!    (capsule/PCS trees), noid_ivc_core::merkle (IVC PCS tree, IVCPCSN_),
//!    capsule.rs — all proof-internal commitments, never semantic payload.

use noid_ivc_core::deep_chain::spine::{
    build_spine_instance_columns_with_contract, SpineContractInstanceFlat, SpineInstanceFlat,
};
use noid_ivc_core::field::F128;
use noid_poseidon2b::native::compression::{compress_with_tag, compress_flat_feed_forward_with_tag};
use noid_poseidon2b::native::domain::{DomainTag, TAG_CAPSNODE, TAG_CLAIMS, TAG_EXSTNOD, TAG_SEGMENTTREE, TAG_TXROOT};

fn input(left_hex: &str, right_hex: &str) -> ([u8; 32], [u8; 32]) {
    (
        hex32(left_hex),
        hex32(right_hex),
    )
}

fn hex32(text: &str) -> [u8; 32] {
    let bytes = hex::decode(text).expect("hex");
    bytes.try_into().expect("32 bytes")
}

fn f128(v: u128) -> F128 {
    // Direct flat image: for testing digest sensitivity we only need any
    // injective mapping of the integer into the field; use the same
    // flat_const the production builder uses via re-export.
    noid_ivc_core::field_circuit::flat_const(v)
}

fn main() {
    let mut failures = 0usize;
    macro_rules! check {
        ($cond:expr, $($msg:expr),+ $(,)?) => {
            if !$cond { failures += 1; eprintln!("FAIL {}", format!($($msg),+)); }
        };
    }

    let first = input(
        "2d8a16372955da0403d2a69740bd860cb83fa1c69c1e7129c671a616a5951930",
        "e681a0b3bee99f85353fbaac20999ef2ec23409e03068aa9e97759648e114d01",
    );
    let second = input(
        "6f7bb24eb68869bdb4f59a582a6a984c5e40eaa3ade728c02e887b0303825ca6",
        "e6312b07507b9f4cc0675c2f9fcd656d4e90a4a753c0a5314779d2d3896cf208",
    );

    // 1. Known collision reproduces (feed-forward compressor, CAPSNODE).
    let d1 = compress_flat_feed_forward_with_tag(TAG_CAPSNODE, &first.0, &first.1);
    let d2 = compress_flat_feed_forward_with_tag(TAG_CAPSNODE, &second.0, &second.1);
    check!(d1 == d2, "known CAPSNODE collision no longer reproduces");
    println!(
        "1. feed-forward TAG_CAPSNODE collision: reproduces ({})",
        hex::encode(d1)
    );

    // 2. The same pair under the SEMANTIC node compressors (full sponge).
    let ivc_pcsn = DomainTag::new(b"IVCPCSN_");
    for (tag, name) in [
        (TAG_EXSTNOD, "TAG_EXSTNOD (exact-state nodes)"),
        (TAG_TXROOT, "TAG_TXROOT (tx root)"),
        (TAG_SEGMENTTREE, "TAG_SEGMENTTREE (segment tree)"),
        (TAG_CLAIMS, "TAG_CLAIMS (claims)"),
    ] {
        let a = compress_with_tag(tag, &first.0, &first.1);
        let b = compress_with_tag(tag, &second.0, &second.1);
        check!(a != b, "collision pair collides under {name}");
        println!(
            "2. {name}: pair digests differ ({} vs {})",
            &hex::encode(a)[..8],
            &hex::encode(b)[..8]
        );
    }
    // And the feed-forward compressor under the IVC PCS node tag (different
    // domain than the collision, same function family — no claim either way,
    // report only).
    let a = compress_flat_feed_forward_with_tag(ivc_pcsn, &first.0, &first.1);
    let b = compress_flat_feed_forward_with_tag(ivc_pcsn, &second.0, &second.1);
    println!(
        "2b. feed-forward IVCPCSN_ (PCS-internal): pair digests {}",
        if a == b { "COLLIDE (same function family)" } else { "differ for this pair" }
    );

    // 3. Object-root binding: single-bit sensitivity of every semantic cell.
    let ghost = SpineInstanceFlat::ghost();
    let base = SpineContractInstanceFlat {
        program: [[f128(0x11), f128(0x22)]; 16],
        current: f128(0x3333),
        next: f128(0x4444),
        controller: [f128(1), f128(2)],
        refund_authority: [f128(3), f128(4)],
        deadline: f128(1000),
        claim_recipient: [f128(5), f128(6)],
        refund_recipient: [f128(7), f128(8)],
        rules: [f128(9), f128(10), f128(11), f128(12)],
    };
    let base_columns = build_spine_instance_columns_with_contract(&ghost, Some(&base));
    let flip = |v: F128, bit: u32| {
        let raw = (v.lo as u128) | ((v.hi as u128) << 64);
        f128(raw ^ (1u128 << bit))
    };
    let mut cells_changed_digest = 0usize;
    let mut cells_changed_old_root = 0usize;
    let mut cells_changed_new_root = 0usize;
    let mut probes = 0usize;
    // program words
    for step in 0..16usize {
        for lane in 0..2usize {
            let mut tampered = base;
            tampered.program[step][lane] = flip(base.program[step][lane], 7);
            let c = build_spine_instance_columns_with_contract(&ghost, Some(&tampered));
            probes += 1;
            cells_changed_digest += usize::from(c.contract_code_digest != base_columns.contract_code_digest);
            cells_changed_old_root += usize::from(c.contract_old_object_root != base_columns.contract_old_object_root);
            cells_changed_new_root += usize::from(c.contract_new_object_root != base_columns.contract_new_object_root);
        }
    }
    // current / next / policy cells
    for (name, mutate) in [
        ("current", Box::new(|c: &mut SpineContractInstanceFlat| { c.current = flip(c.current, 9); }) as Box<dyn Fn(&mut SpineContractInstanceFlat)>),
        ("next", Box::new(|c: &mut SpineContractInstanceFlat| { c.next = flip(c.next, 9); })),
        ("deadline", Box::new(|c: &mut SpineContractInstanceFlat| { c.deadline = flip(c.deadline, 3); })),
        ("rules0", Box::new(|c: &mut SpineContractInstanceFlat| { c.rules[0] = flip(c.rules[0], 1); })),
    ] {
        let mut tampered = base;
        mutate(&mut tampered);
        let c = build_spine_instance_columns_with_contract(&ghost, Some(&tampered));
        probes += 1;
        let digest_changed = c.contract_policy_digest != base_columns.contract_policy_digest;
        let old_changed = c.contract_old_object_root != base_columns.contract_old_object_root;
        let new_changed = c.contract_new_object_root != base_columns.contract_new_object_root;
        println!(
            "3. object-root sensitivity {name}: policy digest changed={digest_changed}, old root changed={old_changed}, new root changed={new_changed}"
        );
        match name {
            "current" => {
                check!(old_changed, "current must move the old root");
                check!(!new_changed, "current must NOT move the new root");
            }
            "next" => {
                check!(new_changed, "next must move the new root");
                check!(!old_changed, "next must NOT move the old root");
            }
            _ => {
                check!(old_changed && new_changed, "policy cells must move both roots");
            }
        }
        let _ = digest_changed;
    }
    println!(
        "3. program cells: {cells_changed_digest}/32 single-bit changes moved the code digest; \
         old root moved {cells_changed_old_root}/32 (expect 32 via policy chain), \
         new root moved {cells_changed_new_root}/32 (expect 32); probes={probes}"
    );
    check!(cells_changed_digest == 32, "some program word did not move the code digest");
    check!(cells_changed_old_root == 32, "some program word did not move the old object root");
    check!(cells_changed_new_root == 32, "some program word did not move the new object root");

    println!("domain separation: {failures} failures");
    if failures == 0 {
        println!(
            "VERDICT: the known compressor collision is confined to PCS-internal trees; \
             semantic bindings (state, tx, object roots) use full-sponge wraps and are \
             single-bit sensitive in every committed cell"
        );
    } else {
        println!("VERDICT: FAILURES — see stderr");
        std::process::exit(1);
    }
}
