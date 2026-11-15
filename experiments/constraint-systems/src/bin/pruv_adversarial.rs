//! Hand-written adversarial Pruv programs — the wrap/underflow/boundary cases
//! a program generator would rarely emit — pushed through the REAL paths:
//!   noid_pruv::compile  ->  native integer-program execution
//!                        ->  the production R1CS circuit (bind_program[_v2_1])
//!                        ->  noid_pruv::verify (the Z3 postcondition checker)
//!
//! Question answered per case: does the constraint system admit a witness the
//! native semantics would reject (wrap, ghost write), and does the Z3
//! postcondition checker agree with the native verdict class?

use std::path::Path;

use noid_core::Block128;
use noid_ivc_core::field_circuit::{FieldR1csBuilder, LinExpr};
use noid_poseidon2b::primitives::Address;
use noid_recursive::acceptance::trace::alloc_block;
use noid_recursive::acceptance::trace::integer_program::{
    bind_program, bind_program_v2_1, ContextTrace,
};
use noid_tx::experimental_object::integer_program as native;
use native::Context;
use serde_json::json;

const Z3: &str = "/opt/homebrew/bin/z3";

fn field(b: &mut FieldR1csBuilder, v: u128) -> LinExpr {
    alloc_block(b, Block128(v))
}

fn build_circuit(
    program: &native::Program,
    current: Block128,
    next: Block128,
    context: &Context,
    cfg: (bool, bool, bool, bool),
) -> bool {
    let (v21, aliases, linear_rows, fused) = cfg;
    let mut b = FieldR1csBuilder::new();
    if aliases {
        b.enable_symbolic_product_aliases();
    }
    if linear_rows {
        b.enable_reused_linear_rows();
    }
    if fused {
        b.enable_fused_boolean_pins();
    }
    let program_w = program.map(|pair| pair.map(|v| field(&mut b, v.0)));
    let current_w = field(&mut b, current.0);
    let next_w = field(&mut b, next.0);
    let ctx = ContextTrace {
        height: field(&mut b, context.height as u128),
        fee: field(&mut b, context.fee as u128),
        payout: field(&mut b, context.payout as u128),
        retained: field(&mut b, context.retained as u128),
        input_amount: field(&mut b, context.input_amount as u128),
        before_deadline: field(&mut b, context.before_deadline as u128),
        terminal: field(&mut b, context.terminal as u128),
        has_payout: field(&mut b, context.has_payout as u128),
        payout_owner: context.payout_owner.map(|v| field(&mut b, v as u128)),
        input_creation_id: field(&mut b, context.input_creation_id as u128),
        input_slot: field(&mut b, context.input_slot as u128),
        retained_slot: field(&mut b, context.retained_slot as u128),
        payout_slot: field(&mut b, context.payout_slot as u128),
    };
    let live_w = LinExpr::from_wire(b.alloc_bool(true));
    if v21 {
        bind_program_v2_1(&mut b, &program_w, &current_w, &next_w, &ctx, &live_w);
    } else {
        bind_program(&mut b, &program_w, &current_w, &next_w, &ctx, &live_w);
    }
    let (matrix, witness) = b.build();
    matrix.satisfies(&witness)
}

const CONFIGS: [(bool, bool, bool, bool); 10] = [
    (false, false, false, false),
    (true, false, false, false),
    (false, true, false, false),
    (false, false, true, false),
    (false, false, false, true),
    (true, true, false, false),
    (true, false, true, false),
    (true, false, false, true),
    (true, true, true, true),
    (false, true, true, true),
];

fn all_configs_reject(
    program: &native::Program,
    current: Block128,
    next: Block128,
    context: &Context,
) -> bool {
    CONFIGS
        .iter()
        .all(|cfg| !build_circuit(program, current, next, context, *cfg))
}

fn all_configs_accept(
    program: &native::Program,
    current: Block128,
    next: Block128,
    context: &Context,
) -> bool {
    CONFIGS
        .iter()
        .all(|cfg| build_circuit(program, current, next, context, *cfg))
}

fn params(extra: &[(&str, &str)]) -> serde_json::Value {
    let mut base = json!({
        "spending_key": Address([1; 32]).to_bech32(),
        "recovery_key": Address([2; 32]).to_bech32(),
        "recipient": Address([3; 32]).to_bech32(),
        "recover_at": "1000",
        "max_fee": "100",
        "max_payment": "10000",
    });
    for (name, value) in extra {
        base.as_object_mut().unwrap().insert((*name).into(), json!(value));
    }
    base
}

/// The budget skeleton with parameterized state inits, call body and ensures.
fn source(state: &str, body: &str, ensures: &str) -> String {
    format!(
        "contract Adversarial {{\n\
        param spending_key: address;\n\
        param recovery_key: address;\n\
        param recipient: address;\n\
        param recover_at: u64;\n\
        param max_fee: u64;\n\
        param max_payment: u64;\n\
        {state}\n\
        policy {{\n\
        claim_authority = spending_key;\n\
        recovery_authority = recovery_key;\n\
        claim_recipient = recipient;\n\
        recovery_recipient = recovery_key;\n\
        deadline_height = recover_at;\n\
        max_fee_micronoid = max_fee;\n\
        max_payout_micronoid = max_payment;\n\
        min_retained_micronoid = 0;\n\
        claim_can_continue = true;\n\
        claim_can_close = false;\n\
        recovery_can_continue = false;\n\
        recovery_can_close = true;\n\
        unrestricted_payout_recipient = false;\n\
        }}\n\
        call {{ {body} }}\n\
        {ensures}\n\
        }}"
    )
}

fn dflt_ctx() -> Context {
    Context {
        height: 500,
        fee: 0,
        payout: 0,
        retained: 0,
        input_amount: 0,
        before_deadline: true,
        terminal: false,
        has_payout: true,
        payout_owner: [0; 4],
        input_creation_id: 0,
        input_slot: 0,
        retained_slot: 0,
        payout_slot: 0,
    }
}

fn main() {
    let z3 = Path::new(Z3);
    let mut failures = 0usize;

    macro_rules! check {
        ($cond:expr, $($msg:expr),+ $(,)?) => {
            if !$cond { failures += 1; eprintln!("FAIL {}", format!($($msg),+)); }
        };
    }

    // ------------------------------------------------------------------
    // P1. State underflow: remaining(1000) -= payout(1005) + fee(0).
    // Native must fail closed; the wrap witness (u64::MAX - 4) must be
    // rejected by every circuit configuration.
    // ------------------------------------------------------------------
    {
        let src = source(
            "state remaining: u64 = 1000;",
            "remaining -= payout + fee;",
            "ensures remaining <= old(remaining);",
        );
        let contract = noid_pruv::compile(&src, &params(&[])).expect("P1 compiles");
        let mut ctx = dflt_ctx();
        ctx.payout = 1005;
        ctx.fee = 0;
        let native_result = native::execute(&contract.opening().program, contract.opening().state, ctx);
        check!(native_result.is_err(), "P1: native accepted underflow");
        // The forger's witness: 1000 - 1005 wrapped = 18446744073709550611.
        let wrapped = native::pack_state([1000u64.wrapping_sub(1005), 0]);
        check!(
            all_configs_reject(&contract.opening().program, contract.opening().state, wrapped, &ctx),
            "P1: circuit accepted wrap witness"
        );
        check!(
            all_configs_reject(&contract.opening().program, contract.opening().state, contract.opening().state, &ctx),
            "P1: circuit accepted no-op witness"
        );
        // Control: an honest, non-underflow execution satisfies.
        let mut ok_ctx = dflt_ctx();
        ok_ctx.payout = 400;
        let ok_next = native::execute(&contract.opening().program, contract.opening().state, ok_ctx)
            .expect("P1 control executes");
        check!(
            all_configs_accept(&contract.opening().program, contract.opening().state, ok_next, &ok_ctx),
            "P1: circuit rejected honest control"
        );
        let verdict = noid_pruv::verify::verify(&contract, z3).expect("P1 z3 runs");
        println!("P1 underflow: native=Err, wrap-witness rejected, control accepted, z3={verdict:?}");
        // With a universally quantified payout, successful executions exist and
        // all satisfy the postcondition: expect Verified.
        check!(
            matches!(verdict, noid_pruv::verify::Verdict::Verified),
            "P1: unexpected z3 verdict {verdict:?}"
        );
    }

    // ------------------------------------------------------------------
    // P2. Counter overflow: calls initialized to u64::MAX, then += 1.
    // ------------------------------------------------------------------
    {
        let src = source(
            "state calls: u64 = 18446744073709551615;",
            "calls += 1;",
            "ensures calls >= old(calls);",
        );
        let contract = noid_pruv::compile(&src, &params(&[])).expect("P2 compiles");
        let ctx = dflt_ctx();
        let native_result = native::execute(&contract.opening().program, contract.opening().state, ctx);
        check!(native_result.is_err(), "P2: native accepted overflow");
        let wrapped = native::pack_state([0, 0]);
        check!(
            all_configs_reject(&contract.opening().program, contract.opening().state, wrapped, &ctx),
            "P2: circuit accepted wrap-to-zero witness"
        );
        check!(
            all_configs_reject(&contract.opening().program, contract.opening().state, contract.opening().state, &ctx),
            "P2: circuit accepted no-op witness"
        );
        let verdict = noid_pruv::verify::verify(&contract, z3).expect("P2 z3 runs");
        println!("P2 overflow: native=Err, wrap-witness rejected, z3={verdict:?}");
        check!(
            matches!(verdict, noid_pruv::verify::Verdict::Verified),
            "P2: unexpected z3 verdict {verdict:?}"
        );
    }

    // ------------------------------------------------------------------
    // P3. Deadline boundary: at height == deadline the guard is inactive and
    // the decrement must NOT happen; the forged decrement is rejected.
    // ------------------------------------------------------------------
    {
        let src = source(
            "state remaining: u64 = 1000;",
            "when before_deadline { remaining -= payout; }",
            "ensures remaining <= old(remaining);",
        );
        let contract = noid_pruv::compile(&src, &params(&[])).expect("P3 compiles");
        let mut ctx = dflt_ctx();
        ctx.height = 1000; // == deadline (recover_at param 1000): strict less-than is false
        ctx.before_deadline = false;
        ctx.payout = 400;
        let honest = native::execute(&contract.opening().program, contract.opening().state, ctx)
            .expect("P3 honest no-op");
        check!(
            honest == contract.opening().state,
            "P3: native decremented past deadline"
        );
        check!(
            all_configs_accept(&contract.opening().program, contract.opening().state, honest, &ctx),
            "P3: circuit rejected honest no-op"
        );
        let forged = native::pack_state([600, 0]);
        check!(
            all_configs_reject(&contract.opening().program, contract.opening().state, forged, &ctx),
            "P3: circuit accepted post-deadline decrement"
        );
        // Boundary one below the deadline executes.
        let mut before = ctx;
        before.height = 999;
        before.before_deadline = true;
        let next = native::execute(&contract.opening().program, contract.opening().state, before)
            .expect("P3 pre-deadline executes");
        check!(
            all_configs_accept(&contract.opening().program, contract.opening().state, next, &before),
            "P3: circuit rejected pre-deadline execution"
        );
        let verdict = noid_pruv::verify::verify(&contract, z3).expect("P3 z3 runs");
        println!("P3 deadline: boundary no-op kept, forged decrement rejected, z3={verdict:?}");
        check!(
            matches!(verdict, noid_pruv::verify::Verdict::Verified),
            "P3: unexpected z3 verdict {verdict:?}"
        );
    }

    // ------------------------------------------------------------------
    // P4. Assertion boundary: require input_amount <= 100; probe 100 and 101.
    // ------------------------------------------------------------------
    {
        let src = source(
            "state seen: u64 = 0;",
            "require input_amount <= 100; seen = input_amount;",
            "ensures seen <= 100;",
        );
        let contract = noid_pruv::compile(&src, &params(&[])).expect("P4 compiles");
        let mut ctx = dflt_ctx();
        ctx.input_amount = 100;
        let ok = native::execute(&contract.opening().program, contract.opening().state, ctx)
            .expect("P4 boundary accepted");
        check!(
            all_configs_accept(&contract.opening().program, contract.opening().state, ok, &ctx),
            "P4: circuit rejected boundary-true assert"
        );
        ctx.input_amount = 101;
        check!(
            native::execute(&contract.opening().program, contract.opening().state, ctx).is_err(),
            "P4: native accepted boundary violation"
        );
        // Forged witness: claim the assignment happened anyway (seen = 101).
        let forged = native::pack_state([101, 0]);
        check!(
            all_configs_reject(&contract.opening().program, contract.opening().state, forged, &ctx),
            "P4: circuit accepted post-assert witness"
        );
        // Forged witness: claim the state kept 0 while skipping the assert
        // failure — i.e. assert passed with input_amount = 101.
        check!(
            all_configs_reject(&contract.opening().program, contract.opening().state, contract.opening().state, &ctx),
            "P4: circuit accepted assert-skip witness"
        );
        let verdict = noid_pruv::verify::verify(&contract, z3).expect("P4 z3 runs");
        println!("P4 assert boundary: 100 ok / 101 rejected everywhere, z3={verdict:?}");
        check!(
            matches!(verdict, noid_pruv::verify::Verdict::Verified),
            "P4: unexpected z3 verdict {verdict:?}"
        );
    }

    // ------------------------------------------------------------------
    // P5. Fail-closed intermediate overflow inside a require: the ADD in
    // `retained + payout` overflows before the assert reads it — the native
    // execution fails closed (no silent modular pass).
    // ------------------------------------------------------------------
    {
        let src = source(
            "state ok: u64 = 0;",
            "require retained + payout >= payout; ok = 1;",
            "ensures ok == 1;",
        );
        let contract = noid_pruv::compile(&src, &params(&[])).expect("P5 compiles");
        let mut ctx = dflt_ctx();
        ctx.retained = u64::MAX;
        ctx.payout = 1;
        check!(
            native::execute(&contract.opening().program, contract.opening().state, ctx).is_err(),
            "P5: native accepted overflowing intermediate"
        );
        // Wrapped intermediate: retained + payout = 0 >= payout(1) is FALSE,
        // so a silent-wrap semantics would reject — try the "assert passed"
        // witness (ok = 1) and the no-op witness; both must be rejected.
        let forged_ok = native::pack_state([1, 0]);
        check!(
            all_configs_reject(&contract.opening().program, contract.opening().state, forged_ok, &ctx),
            "P5: circuit accepted post-overflow witness"
        );
        check!(
            all_configs_reject(&contract.opening().program, contract.opening().state, contract.opening().state, &ctx),
            "P5: circuit accepted no-op witness"
        );
        // Control: non-overflowing values pass.
        ctx.retained = 5;
        ctx.payout = 3;
        let ok = native::execute(&contract.opening().program, contract.opening().state, ctx)
            .expect("P5 control executes");
        check!(
            all_configs_accept(&contract.opening().program, contract.opening().state, ok, &ctx),
            "P5: circuit rejected honest control"
        );
        let verdict = noid_pruv::verify::verify(&contract, z3).expect("P5 z3 runs");
        println!("P5 intermediate overflow: fail-closed everywhere, z3={verdict:?}");
        // Universally quantified: every successful execution has
        // retained + payout >= payout (unsigned), so expect Verified.
        check!(
            matches!(verdict, noid_pruv::verify::Verdict::Verified),
            "P5: expected Verified, got {verdict:?}"
        );
    }

    // ------------------------------------------------------------------
    // P6. A genuine counterexample through the real Z3 path (control for the
    // checker itself: it must be able to say Counterexample).
    // ------------------------------------------------------------------
    {
        let src = source(
            "state calls: u64 = 0;",
            "calls += 1;",
            "ensures calls <= 0;",
        );
        let contract = noid_pruv::compile(&src, &params(&[])).expect("P6 compiles");
        let verdict = noid_pruv::verify::verify(&contract, z3).expect("P6 z3 runs");
        println!("P6 counterexample control: z3={verdict:?} (expected Counterexample)");
        check!(
            matches!(verdict, noid_pruv::verify::Verdict::Counterexample),
            "P6: expected Counterexample, got {verdict:?}"
        );
    }

    // ------------------------------------------------------------------
    // P7. Infeasibility control: require false.
    // ------------------------------------------------------------------
    {
        let src = source(
            "state x: u64 = 0;",
            "require 2 == 3; x = 1;",
            "ensures x == 1;",
        );
        let contract = noid_pruv::compile(&src, &params(&[])).expect("P7 compiles");
        let verdict = noid_pruv::verify::verify(&contract, z3).expect("P7 z3 runs");
        println!("P7 infeasible control: z3={verdict:?} (expected Infeasible)");
        check!(
            matches!(verdict, noid_pruv::verify::Verdict::Infeasible),
            "P7: expected Infeasible, got {verdict:?}"
        );
    }

    // ------------------------------------------------------------------
    // P8. Derived (non-direct) when-condition: the guard is a computed
    // LessThan in a scratch register, not a context boolean — the compiled
    // reset-then-guard path. Forged execution under a false guard rejected.
    // ------------------------------------------------------------------
    {
        let src = source(
            "state remaining: u64 = 1000;",
            "when input_amount < 100 { remaining -= payout; }",
            "ensures remaining <= old(remaining);",
        );
        let contract = noid_pruv::compile(&src, &params(&[])).expect("P8 compiles");
        let mut ctx = dflt_ctx();
        ctx.input_amount = 100; // guard false (strict)
        ctx.payout = 400;
        let honest = native::execute(&contract.opening().program, contract.opening().state, ctx)
            .expect("P8 honest no-op");
        check!(
            honest == contract.opening().state,
            "P8: native decremented under false derived guard"
        );
        check!(
            all_configs_accept(&contract.opening().program, contract.opening().state, honest, &ctx),
            "P8: circuit rejected honest no-op"
        );
        check!(
            all_configs_reject(
                &contract.opening().program,
                contract.opening().state,
                native::pack_state([600, 0]),
                &ctx
            ),
            "P8: circuit accepted forged guarded write"
        );
        ctx.input_amount = 99; // guard true
        let next = native::execute(&contract.opening().program, contract.opening().state, ctx)
            .expect("P8 guarded execution");
        check!(
            all_configs_accept(&contract.opening().program, contract.opening().state, next, &ctx),
            "P8: circuit rejected honest guarded execution"
        );
        let verdict = noid_pruv::verify::verify(&contract, z3).expect("P8 z3 runs");
        println!("P8 derived guard: false-guard write rejected, boundary 99/100 honored, z3={verdict:?}");
        check!(
            matches!(verdict, noid_pruv::verify::Verdict::Verified),
            "P8: unexpected z3 verdict {verdict:?}"
        );
    }

    // ------------------------------------------------------------------
    // P9. Nested when with an INACTIVE parent: the compile.rs
    // reset-before-guard Move must leave the child guard false, so the
    // inner body cannot fire when the outer condition is false.
    // ------------------------------------------------------------------
    {
        let src = source(
            "state remaining: u64 = 1000;",
            "when before_deadline { when input_amount < 100 { remaining -= payout; } }",
            "ensures remaining <= old(remaining);",
        );
        let contract = noid_pruv::compile(&src, &params(&[])).expect("P9 compiles");
        let mut ctx = dflt_ctx();
        ctx.height = 1000; // == deadline: outer guard false
        ctx.before_deadline = false;
        ctx.input_amount = 50; // inner guard would be true
        ctx.payout = 400;
        let honest = native::execute(&contract.opening().program, contract.opening().state, ctx)
            .expect("P9 honest no-op");
        check!(
            honest == contract.opening().state,
            "P9: native fired inner body under inactive parent"
        );
        check!(
            all_configs_accept(&contract.opening().program, contract.opening().state, honest, &ctx),
            "P9: circuit rejected honest no-op"
        );
        check!(
            all_configs_reject(
                &contract.opening().program,
                contract.opening().state,
                native::pack_state([600, 0]),
                &ctx
            ),
            "P9: circuit accepted forged nested write"
        );
        let verdict = noid_pruv::verify::verify(&contract, z3).expect("P9 z3 runs");
        println!("P9 nested when: inactive parent leaves child guard false, z3={verdict:?}");
        check!(
            matches!(verdict, noid_pruv::verify::Verdict::Verified),
            "P9: unexpected z3 verdict {verdict:?}"
        );
    }

    println!(
        "Pruv adversarial: {} failures",
        failures
    );
    if failures == 0 {
        println!("VERDICT: all hand-written wrap/boundary programs fail closed in native, circuit and Z3 checker");
    } else {
        println!("VERDICT: FAILURES — see stderr");
        std::process::exit(1);
    }
}
