//! Witness-uniqueness / under-constraint battery for the Parano1d integer
//! program circuit (the constraint system Pruv compiles into).
//!
//! Method: build the REAL R1CS through `FieldR1csBuilder` +
//! `bind_program`/`bind_program_v2_1` (the production constraint code), then
//! feed candidate FORGERY witnesses (wrap-arithmetic executions, non-boolean
//! predicates, inactive-instruction writes, bit-flipped next states, tampered
//! contexts, invalid encodings) and require `matrix.satisfies()` to reject
//! every one of them. Any satisfaction of a forgery witness is an accepted
//! semantic forgery: two distinct witnesses, one constraint set.
//!
//! Runs every case under 4 builder configurations (plain, and the three
//! production optimizations combined: symbolic product aliases, reused
//! linear rows, fused boolean pins) x 2 arithmetic variants (v1 full, v2.1
//! compact) — the v2.1 variant is selected by ScheduledV2 profiles and has
//! no direct unit tests in the tree.

use std::collections::HashMap;

use noid_core::Block128;
use noid_ivc_core::field_circuit::{FieldR1csBuilder, LinExpr};
use noid_recursive::acceptance::trace::alloc_block;
use noid_recursive::acceptance::trace::integer_program::{
    bind_program, bind_program_v2_1, ContextTrace,
};
use noid_tx::experimental_object::integer_program as native;
use native::{
    Context, Instruction, Opcode, Operand, Predicate, PredicateSource, Register, Program,
};

const CONFIGS: [(bool, bool, bool, bool); 10] = [
    // (v2.1 arithmetic, product aliases, reused linear rows, fused pins)
    (false, false, false, false),
    (true, false, false, false),
    // Each optimization alone, both arithmetic variants (deployments
    // wire-encode the flags individually — runtime_parts_codec/capacity.rs).
    (false, true, false, false),
    (false, false, true, false),
    (false, false, false, true),
    (true, true, false, false),
    (true, false, true, false),
    (true, false, false, true),
    // All optimizations combined.
    (true, true, true, true),
    (false, true, true, true),
];

struct Tally {
    checks: usize,
    failures: usize,
}

impl Tally {
    fn fail(&mut self, message: String) {
        self.failures += 1;
        eprintln!("FAIL {message}");
    }
}

fn field(b: &mut FieldR1csBuilder, v: u128) -> LinExpr {
    alloc_block(b, Block128(v))
}

/// Build the real constraint system for (program, current, next, context,
/// live) under one builder configuration. Returns (satisfies, wires, digest).
fn build(
    program: &Program,
    current: Block128,
    next: Block128,
    context: &Context,
    live: bool,
    cfg: (bool, bool, bool, bool),
) -> (bool, usize, [u8; 32]) {
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
    let live_w = LinExpr::from_wire(b.alloc_bool(live));
    if v21 {
        bind_program_v2_1(&mut b, &program_w, &current_w, &next_w, &ctx, &live_w);
    } else {
        bind_program(&mut b, &program_w, &current_w, &next_w, &ctx, &live_w);
    }
    let wires = b.num_wires();
    let (matrix, witness) = b.build();
    (
        matrix.satisfies(&witness),
        wires,
        matrix.structural_statement_digest(),
    )
}

fn cfg_key(cfg: (bool, bool, bool, bool), live: bool) -> [u8; 8] {
    [cfg.0 as u8, cfg.1 as u8, cfg.2 as u8, cfg.3 as u8, live as u8, 0, 0, 0]
}

fn expect_unsat(
    t: &mut Tally,
    digests: &mut HashMap<[u8; 8], [u8; 32]>,
    label: &str,
    program: &Program,
    current: Block128,
    next: Block128,
    context: &Context,
) {
    for cfg in CONFIGS {
        let (sat, _, digest) = build(program, current, next, context, true, cfg);
        let key = cfg_key(cfg, true);
        let fixed = digests.entry(key).or_insert(digest);
        if *fixed != digest {
            t.fail(format!("[shape fixity] {label} cfg={cfg:?}"));
        }
        if sat {
            t.fail(format!("[forgery accepted] {label} cfg={cfg:?}"));
        }
    }
    t.checks += CONFIGS.len();
}

fn expect_sat(
    t: &mut Tally,
    label: &str,
    program: &Program,
    current: Block128,
    next: Block128,
    context: &Context,
) {
    for cfg in CONFIGS {
        let (sat, _, _) = build(program, current, next, context, true, cfg);
        if !sat {
            t.fail(format!("[honest rejected] {label} cfg={cfg:?}"));
        }
    }
    t.checks += CONFIGS.len();
}

// ---------------------------------------------------------------------------
// Lying interpreters: produce the register evolution a forger *wishes* the
// semantics had. Each is a candidate alternate witness.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Debug)]
#[allow(dead_code)] // Honest documents the reference member of the family.
enum Semantics {
    Honest,
    /// Add wraps mod 2^64 instead of failing.
    WrapAdd,
    /// Subtract wraps mod 2^64 instead of failing.
    WrapSub,
    /// A non-boolean predicate (>1) counts as true instead of erroring.
    PredNonBooleanAsTrue,
    /// A non-boolean predicate (>1) counts as false instead of erroring.
    PredNonBooleanAsFalse,
}

fn execute_with(
    program: &Program,
    current: Block128,
    context: Context,
    sem: Semantics,
) -> Result<Block128, native::ProgramError> {
    native::validate(program)?;
    let state = native::unpack_state(current);
    let mut registers = [state[0], state[1], 0, 0];
    for (step, fields) in program.iter().copied().enumerate() {
        let instruction = Instruction::from_fields(fields).expect("validated program");
        // `Context::predicates` is crate-private; mirror its public encoding.
        let predicates = [
            1u64,
            u64::from(context.before_deadline),
            u64::from(context.terminal),
            u64::from(context.has_payout),
            registers[2],
            registers[3],
        ];
        let raw = predicates[instruction.predicate.source as usize];
        let active = if raw > 1 {
            match sem {
                Semantics::Honest => {
                    return Err(native::ProgramError::NonBooleanPredicate { step })
                }
                Semantics::PredNonBooleanAsTrue => true,
                Semantics::PredNonBooleanAsFalse => false,
                _ => return Err(native::ProgramError::NonBooleanPredicate { step }),
            }
        } else {
            (raw == 1) != instruction.predicate.inverted
        };
        if !active {
            continue;
        }
        let operands = context.operands(registers, instruction.immediate);
        let left = operands[instruction.left as usize];
        let right = operands[instruction.right as usize];
        let destination = instruction.destination as usize;
        registers[destination] = match instruction.opcode {
            Opcode::Keep => registers[destination],
            Opcode::Move => left,
            Opcode::Add => match sem {
                Semantics::WrapAdd => left.wrapping_add(right),
                _ => left
                    .checked_add(right)
                    .ok_or(native::ProgramError::Overflow { step })?,
            },
            Opcode::Subtract => match sem {
                Semantics::WrapSub => left.wrapping_sub(right),
                _ => left
                    .checked_sub(right)
                    .ok_or(native::ProgramError::Underflow { step })?,
            },
            Opcode::Min => left.min(right),
            Opcode::Max => left.max(right),
            Opcode::LessThan => u64::from(left < right),
            Opcode::Equal => u64::from(left == right),
            Opcode::AssertEqual => {
                if left != right {
                    return Err(native::ProgramError::Assertion { step });
                }
                registers[destination]
            }
            Opcode::AssertLessOrEqual => {
                if left > right {
                    return Err(native::ProgramError::Assertion { step });
                }
                registers[destination]
            }
        };
    }
    Ok(native::pack_state([registers[0], registers[1]]))
}

// ---------------------------------------------------------------------------
// Tiny seeded RNG (no external deps).
// ---------------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn interesting_u64(rng: &mut Rng) -> u64 {
    match rng.below(8) {
        0 => 0,
        1 => 1,
        2 => 2,
        3 => u64::MAX - 1,
        4 => u64::MAX,
        5 => 1 << 63,
        6 => (1 << 63) - 1,
        _ => rng.next(),
    }
}

fn register_of(value: u64) -> Register {
    match value {
        0 => Register::State0,
        1 => Register::State1,
        2 => Register::Scratch0,
        _ => Register::Scratch1,
    }
}

fn random_program(rng: &mut Rng) -> Program {
    let mut program = native::EMPTY_PROGRAM;
    for step in 0..native::PROGRAM_STEPS {
        let opcode = Opcode::ALL[rng.below(10) as usize];
        let destination = register_of(rng.below(4));
        // Bias assert opcodes toward identical operands so more random
        // programs execute successfully (honest paths need coverage too).
        let left = Operand::ALL[rng.below(24) as usize];
        let right = match opcode {
            Opcode::AssertEqual | Opcode::AssertLessOrEqual if rng.below(10) < 7 => left,
            _ if rng.below(4) == 0 => left,
            _ => Operand::ALL[rng.below(24) as usize],
        };
        let source = PredicateSource::ALL[rng.below(6) as usize];
        let inverted = rng.below(2) == 1;
        let immediate = interesting_u64(rng);
        program[step] = Instruction::new(opcode, destination, left, right, immediate)
            .when(Predicate { source, inverted })
            .to_fields();
    }
    program
}

fn random_context(rng: &mut Rng) -> Context {
    Context {
        height: interesting_u64(rng),
        fee: interesting_u64(rng),
        payout: interesting_u64(rng),
        retained: interesting_u64(rng),
        input_amount: interesting_u64(rng),
        before_deadline: rng.below(2) == 1,
        terminal: rng.below(2) == 1,
        has_payout: rng.below(2) == 1,
        payout_owner: [
            interesting_u64(rng),
            interesting_u64(rng),
            interesting_u64(rng),
            interesting_u64(rng),
        ],
        input_creation_id: interesting_u64(rng),
        input_slot: (rng.next() >> 32) as u32,
        retained_slot: (rng.next() >> 32) as u32,
        payout_slot: (rng.next() >> 32) as u32,
    }
}

// ---------------------------------------------------------------------------

fn main() {
    let mut t = Tally {
        checks: 0,
        failures: 0,
    };
    let mut digests: HashMap<[u8; 8], [u8; 32]> = HashMap::new();

    // ------------------------------------------------------------------
    // 1. Random differential: native vs circuit, plus forgery candidates.
    // ------------------------------------------------------------------
    let mut rng = Rng(0xC0FF_EE20_2611_1500);
    let mut ok_instances = 0usize;
    let mut err_instances = 0usize;
    let mut lies_rejected = 0usize;
    for index in 0..120usize {
        let program = random_program(&mut rng);
        let current = native::pack_state([interesting_u64(&mut rng), interesting_u64(&mut rng)]);
        let context = random_context(&mut rng);
        let honest = native::execute(&program, current, context);
        match honest {
            Ok(next) => {
                ok_instances += 1;
                expect_sat(&mut t, &format!("random[{index}] honest"), &program, current, next, &context);
                let mut flip_rng = Rng(0x5EED_0000 + index as u64);
                for _ in 0..8 {
                    let bit = flip_rng.below(128);
                    let forged = Block128(next.0 ^ (1u128 << bit));
                    expect_unsat(
                        &mut t,
                        &mut digests,
                        &format!("random[{index}] next-flip bit {bit}"),
                        &program,
                        current,
                        forged,
                        &context,
                    );
                    lies_rejected += 1;
                }
            }
            Err(_) => {
                err_instances += 1;
                for candidate in [
                    current,
                    Block128(current.0 ^ 1),
                    Block128(!current.0),
                    native::pack_state([u64::MAX, u64::MAX]),
                ] {
                    expect_unsat(
                        &mut t,
                        &mut digests,
                        &format!("random[{index}] native-error candidate"),
                        &program,
                        current,
                        candidate,
                        &context,
                    );
                    lies_rejected += 1;
                }
            }
        }
        for sem in [
            Semantics::WrapAdd,
            Semantics::WrapSub,
            Semantics::PredNonBooleanAsTrue,
            Semantics::PredNonBooleanAsFalse,
        ] {
            let Ok(lied) = execute_with(&program, current, context, sem) else {
                continue;
            };
            let differs = match &honest {
                Ok(next) => *next != lied,
                Err(_) => true,
            };
            if differs {
                expect_unsat(
                    &mut t,
                    &mut digests,
                    &format!("random[{index}] lie {sem:?}"),
                    &program,
                    current,
                    lied,
                    &context,
                );
                lies_rejected += 1;
            }
        }
    }
    println!(
        "random differential: {ok_instances} ok / {err_instances} error instances; \
         {lies_rejected} forgery candidates rejected"
    );

    // ------------------------------------------------------------------
    // 2. Directed cases.
    // ------------------------------------------------------------------
    let dflt = Context {
        height: 100,
        fee: 3,
        payout: 47,
        retained: 51,
        input_amount: 101,
        before_deadline: true,
        terminal: false,
        has_payout: true,
        payout_owner: [11, 22, 33, 44],
        input_creation_id: 12345,
        input_slot: 300,
        retained_slot: 301,
        payout_slot: 302,
    };

    // 2a. Add overflow at the u64 boundary.
    {
        let mut program = native::EMPTY_PROGRAM;
        program[0] = Instruction::new(
            Opcode::Add,
            Register::State0,
            Operand::State0,
            Operand::Immediate,
            1,
        )
        .to_fields();
        let current = native::pack_state([u64::MAX, 7]);
        assert!(native::execute(&program, current, dflt).is_err());
        expect_unsat(&mut t, &mut digests, "overflow add: wrapped witness 0", &program, current, native::pack_state([0, 7]), &dflt);
        expect_unsat(&mut t, &mut digests, "overflow add: claim state unchanged", &program, current, current, &dflt);
    }

    // 2b. Subtract underflow.
    {
        let mut program = native::EMPTY_PROGRAM;
        program[0] = Instruction::new(
            Opcode::Subtract,
            Register::State0,
            Operand::State0,
            Operand::Immediate,
            1,
        )
        .to_fields();
        let current = native::pack_state([0, 7]);
        assert!(native::execute(&program, current, dflt).is_err());
        expect_unsat(&mut t, &mut digests, "underflow sub: wrapped witness u64::MAX", &program, current, native::pack_state([u64::MAX, 7]), &dflt);
        expect_unsat(&mut t, &mut digests, "underflow sub: claim state unchanged", &program, current, current, &dflt);
    }

    // 2c. Non-boolean predicate: scratch = 2 then predicate on Scratch0.
    {
        let mut program = native::EMPTY_PROGRAM;
        program[0] = Instruction::new(
            Opcode::Move,
            Register::Scratch0,
            Operand::Immediate,
            Operand::Zero,
            2,
        )
        .to_fields();
        program[1] = Instruction::new(
            Opcode::Move,
            Register::State0,
            Operand::One,
            Operand::Zero,
            0,
        )
        .when(Predicate {
            source: PredicateSource::Scratch0,
            inverted: false,
        })
        .to_fields();
        let current = native::pack_state([5, 9]);
        assert!(native::execute(&program, current, dflt).is_err());
        expect_unsat(&mut t, &mut digests, "non-boolean predicate: as-if-true witness", &program, current, native::pack_state([1, 9]), &dflt);
        expect_unsat(&mut t, &mut digests, "non-boolean predicate: as-if-false witness", &program, current, current, &dflt);
        // The same program with a boolean scratch value is fine.
        program[0] = Instruction::new(
            Opcode::Move,
            Register::Scratch0,
            Operand::Immediate,
            Operand::Zero,
            1,
        )
        .to_fields();
        let honest = native::execute(&program, current, dflt).unwrap();
        expect_sat(&mut t, "boolean predicate honest", &program, current, honest, &dflt);
    }

    // 2d. Inactive instruction must not write.
    {
        let mut program = native::EMPTY_PROGRAM;
        program[0] = Instruction::new(
            Opcode::Move,
            Register::State0,
            Operand::Immediate,
            Operand::Zero,
            77,
        )
        .when(Predicate {
            source: PredicateSource::Terminal,
            inverted: false,
        })
        .to_fields();
        let current = native::pack_state([11, u64::MAX]);
        let honest = native::execute(&program, current, dflt).unwrap();
        assert_eq!(honest, current);
        expect_sat(&mut t, "inactive instruction honest no-op", &program, current, honest, &dflt);
        expect_unsat(&mut t, &mut digests, "inactive instruction: forged write", &program, current, native::pack_state([77, u64::MAX]), &dflt);
        program[0] = Instruction::new(
            Opcode::Move,
            Register::State0,
            Operand::Immediate,
            Operand::Zero,
            77,
        )
        .when(Predicate {
            source: PredicateSource::Terminal,
            inverted: true,
        })
        .to_fields();
        let honest = native::execute(&program, current, dflt).unwrap();
        expect_sat(&mut t, "active inverted predicate honest", &program, current, honest, &dflt);
        expect_unsat(&mut t, &mut digests, "active inverted predicate: forged skip", &program, current, current, &dflt);
    }

    // 2e. Invalid instruction encodings must be unsatisfiable (live=1).
    {
        let base = native::EMPTY_PROGRAM;
        let current = native::pack_state([3, 4]);
        for word in [
            10u128,
            11,
            15,
            24 << 6,
            31 << 6,
            24 << 11,
            31 << 11,
            6 << 16,
            7 << 16,
            1 << native::INSTRUCTION_BITS,
            u128::MAX,
        ] {
            let mut program = base;
            program[0][0] = Block128(word);
            expect_unsat(
                &mut t,
                &mut digests,
                &format!("invalid descriptor word {word:#x}"),
                &program,
                current,
                current,
                &dflt,
            );
        }
        let mut program = base;
        program[0][1] = Block128(1u128 << 64);
        expect_unsat(&mut t, &mut digests, "immediate beyond u64", &program, current, current, &dflt);
    }

    // 2f. Context substitution, component level.
    {
        let mut program = native::EMPTY_PROGRAM;
        program[0] = Instruction::new(
            Opcode::Move,
            Register::State0,
            Operand::Height,
            Operand::Zero,
            0,
        )
        .to_fields();
        let current = native::pack_state([1, 2]);
        let ctx900 = Context { height: 900, ..dflt };
        let honest = native::execute(&program, current, ctx900).unwrap();
        expect_sat(&mut t, "height honest 900", &program, current, honest, &ctx900);
        let ctx901 = Context { height: 901, ..dflt };
        expect_unsat(&mut t, &mut digests, "height tampered 901 with stale next", &program, current, honest, &ctx901);
        let mut consistent_sat = 0;
        for cfg in CONFIGS {
            let (sat, _, _) = build(&program, current, native::pack_state([901, 2]), &ctx901, true, cfg);
            consistent_sat += usize::from(sat);
        }
        println!(
            "context advice (component level): consistent height tamper satisfies \
             {consistent_sat}/{} configs (expected: all — binding lives one level up)",
            CONFIGS.len()
        );
        if consistent_sat != CONFIGS.len() {
            t.fail("consistent context tamper unexpectedly rejected".into());
        }
        t.checks += CONFIGS.len();
    }

    // 2g. Dead slot (live = 0): document that garbage next is accepted.
    {
        let mut program = native::EMPTY_PROGRAM;
        program[0] = Instruction::new(
            Opcode::Move,
            Register::State0,
            Operand::Immediate,
            Operand::Zero,
            77,
        )
        .to_fields();
        let current = native::pack_state([1, 2]);
        let mut ghost_sat = 0;
        for cfg in CONFIGS {
            let (sat, _, _) = build(&program, current, Block128(0xDEAD_BEEF_CAFE_F00D), &dflt, false, cfg);
            ghost_sat += usize::from(sat);
        }
        println!(
            "dead slot (live = 0): garbage next satisfies {ghost_sat}/{} configs \
             (expected: all — ghost by design; live is an authenticated bitmap bit)",
            CONFIGS.len()
        );
        if ghost_sat != CONFIGS.len() {
            t.fail("dead-slot ghost unexpectedly constrained".into());
        }
        t.checks += CONFIGS.len();
    }

    // 2h. Exhaustive 128 single-bit flips on a dedicated non-trivial program.
    {
        let mut program = native::EMPTY_PROGRAM;
        program[0] = Instruction::new(
            Opcode::Add,
            Register::State0,
            Operand::State0,
            Operand::Height,
            30,
        )
        .when(Predicate::BEFORE_DEADLINE)
        .to_fields();
        program[1] = Instruction::new(
            Opcode::Subtract,
            Register::State1,
            Operand::State1,
            Operand::Fee,
            0,
        )
        .to_fields();
        program[2] = Instruction::new(
            Opcode::Min,
            Register::State0,
            Operand::State0,
            Operand::Retained,
            0,
        )
        .to_fields();
        program[3] = Instruction::new(
            Opcode::Max,
            Register::State1,
            Operand::State1,
            Operand::Payout,
            0,
        )
        .to_fields();
        let current = native::pack_state([1000, 3]);
        let next = native::execute(&program, current, dflt).unwrap();
        for bit in 0..128u32 {
            expect_unsat(
                &mut t,
                &mut digests,
                &format!("exhaustive flip bit {bit}"),
                &program,
                current,
                Block128(next.0 ^ (1u128 << bit)),
                &dflt,
            );
        }
        expect_sat(&mut t, "four-op program honest", &program, current, next, &dflt);
    }

    println!("TOTAL: {} build-and-check configurations, {} failures", t.checks, t.failures);
    if t.failures == 0 {
        println!(
            "VERDICT: witness uniqueness held on every probed constraint set; \
             no alternate satisfying witness found"
        );
    } else {
        println!("VERDICT: FAILURES — see stderr");
        std::process::exit(1);
    }
}
