use noid_poseidon2b::primitives::Address;
use noid_tx::experimental_object::integer_program::{self as native, Context};
use serde_json::json;

#[derive(Clone, Debug)]
enum Expr {
    U64(u64),
    Bool(bool),
    State(usize),
    Context(&'static str),
    Not(Box<Expr>),
    Binary(&'static str, Box<Expr>, Box<Expr>),
}

#[derive(Clone, Debug)]
enum Stmt {
    Assign(usize, Expr),
    Require(Expr),
    When(Expr, Vec<Stmt>),
}

#[derive(Clone, Debug)]
enum Value {
    U64(u64),
    Bool(bool),
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        self.next() as usize % n
    }
}

fn u64_expr(rng: &mut Rng, depth: usize) -> Expr {
    let atoms = [
        Expr::U64(0),
        Expr::U64(1),
        Expr::U64(2),
        Expr::U64(u64::MAX),
        Expr::State(0),
        Expr::State(1),
        Expr::Context("height"),
        Expr::Context("fee"),
        Expr::Context("payout"),
        Expr::Context("retained"),
        Expr::Context("input_amount"),
        Expr::Context("payout_owner0"),
        Expr::Context("input_creation_id"),
        Expr::Context("input_slot"),
        Expr::Context("retained_slot"),
        Expr::Context("payout_slot"),
    ];
    if depth == 0 || rng.below(3) == 0 {
        return atoms[rng.below(atoms.len())].clone();
    }
    match rng.below(4) {
        0 => Expr::Binary(
            "min",
            Box::new(u64_expr(rng, depth - 1)),
            Box::new(u64_expr(rng, depth - 1)),
        ),
        1 => Expr::Binary(
            "max",
            Box::new(u64_expr(rng, depth - 1)),
            Box::new(u64_expr(rng, depth - 1)),
        ),
        2 => Expr::Binary(
            "+",
            Box::new(u64_expr(rng, depth - 1)),
            Box::new(u64_expr(rng, depth - 1)),
        ),
        _ => Expr::Binary(
            "-",
            Box::new(u64_expr(rng, depth - 1)),
            Box::new(u64_expr(rng, depth - 1)),
        ),
    }
}

fn bool_expr(rng: &mut Rng, depth: usize) -> Expr {
    if depth > 0 && rng.below(5) == 0 {
        return match rng.below(6) {
            0 => Expr::Bool(true),
            1 => Expr::Bool(false),
            2 => Expr::Context("before_deadline"),
            3 => Expr::Context("after_deadline"),
            4 => Expr::Context("terminal"),
            _ => Expr::Context("has_payout"),
        };
    }
    if depth > 0 && rng.below(4) == 0 {
        return Expr::Not(Box::new(bool_expr(rng, depth - 1)));
    }
    let op = ["==", "!=", "<", "<=", ">", ">="][rng.below(6)];
    Expr::Binary(
        op,
        Box::new(u64_expr(rng, depth.saturating_sub(1))),
        Box::new(u64_expr(rng, depth.saturating_sub(1))),
    )
}

fn statements(rng: &mut Rng, count: usize, depth: usize) -> Vec<Stmt> {
    (0..count)
        .map(|_| match rng.below(if depth == 0 { 3 } else { 4 }) {
            0 | 1 => Stmt::Assign(rng.below(2), u64_expr(rng, 2)),
            2 => Stmt::Require(bool_expr(rng, 2)),
            _ => {
                let count = 1 + rng.below(2);
                Stmt::When(bool_expr(rng, 2), statements(rng, count, depth - 1))
            }
        })
        .collect()
}

fn print_expr(expr: &Expr) -> String {
    match expr {
        Expr::U64(value) => value.to_string(),
        Expr::Bool(value) => value.to_string(),
        Expr::State(index) => ["remaining", "calls"][*index].to_owned(),
        Expr::Context(name) => (*name).to_owned(),
        Expr::Not(inner) => format!("!({})", print_expr(inner)),
        Expr::Binary(op, left, right) if ["min", "max"].contains(op) => {
            format!("{}({}, {})", op, print_expr(left), print_expr(right))
        }
        Expr::Binary(op, left, right) => {
            format!("({} {} {})", print_expr(left), op, print_expr(right))
        }
    }
}

fn print_statements(statements: &[Stmt], indent: usize) -> String {
    let pad = " ".repeat(indent);
    let mut out = String::new();
    for statement in statements {
        match statement {
            Stmt::Assign(index, expr) => out.push_str(&format!(
                "{pad}{} = {};\n",
                ["remaining", "calls"][*index],
                print_expr(expr)
            )),
            Stmt::Require(expr) => out.push_str(&format!("{pad}require {};\n", print_expr(expr))),
            Stmt::When(condition, body) => {
                out.push_str(&format!("{pad}when {} {{\n", print_expr(condition)));
                out.push_str(&print_statements(body, indent + 4));
                out.push_str(&format!("{pad}}}\n"));
            }
        }
    }
    out
}

fn source(statements: &[Stmt]) -> String {
    format!(
        "contract Fuzz {{\n    param a: address;\n    param b: address;\n    param c: address;\n    param d: address;\n    state remaining: u64 = 7;\n    state calls: u64 = 11;\n    policy {{\n        claim_authority = a;\n        recovery_authority = b;\n        claim_recipient = c;\n        recovery_recipient = d;\n        deadline_height = 100;\n        max_fee_micronoid = 1000000;\n        max_payout_micronoid = 1000000;\n        min_retained_micronoid = 0;\n        claim_can_continue = true;\n        claim_can_close = true;\n        recovery_can_continue = true;\n        recovery_can_close = true;\n        unrestricted_payout_recipient = true;\n    }}\n    call {{\n{}    }}\n}}\n",
        print_statements(statements, 8)
    )
}

fn parameters() -> serde_json::Value {
    json!({
        "a": Address([1; 32]).to_bech32(),
        "b": Address([2; 32]).to_bech32(),
        "c": Address([3; 32]).to_bech32(),
        "d": Address([4; 32]).to_bech32(),
    })
}

fn context_value(name: &str, states: [u64; 2], context: Context) -> Value {
    match name {
        "remaining" => Value::U64(states[0]),
        "calls" => Value::U64(states[1]),
        "height" => Value::U64(context.height),
        "fee" => Value::U64(context.fee),
        "payout" => Value::U64(context.payout),
        "retained" => Value::U64(context.retained),
        "input_amount" => Value::U64(context.input_amount),
        "before_deadline" => Value::Bool(context.before_deadline),
        "after_deadline" => Value::Bool(!context.before_deadline),
        "terminal" => Value::Bool(context.terminal),
        "has_payout" => Value::Bool(context.has_payout),
        "payout_owner0" => Value::U64(context.payout_owner[0]),
        "input_creation_id" => Value::U64(context.input_creation_id),
        "input_slot" => Value::U64(context.input_slot.into()),
        "retained_slot" => Value::U64(context.retained_slot.into()),
        "payout_slot" => Value::U64(context.payout_slot.into()),
        _ => unreachable!(),
    }
}

fn eval(expr: &Expr, states: [u64; 2], context: Context) -> Result<Value, ()> {
    Ok(match expr {
        Expr::U64(value) => Value::U64(*value),
        Expr::Bool(value) => Value::Bool(*value),
        Expr::State(index) => Value::U64(states[*index]),
        Expr::Context(name) => context_value(name, states, context),
        Expr::Not(inner) => match eval(inner, states, context)? {
            Value::Bool(value) => Value::Bool(!value),
            _ => return Err(()),
        },
        Expr::Binary(op, left, right) => {
            let left = eval(left, states, context)?;
            let right = eval(right, states, context)?;
            if ["min", "max"].contains(op) {
                let (Value::U64(left), Value::U64(right)) = (left, right) else {
                    return Err(());
                };
                Value::U64(if *op == "min" {
                    left.min(right)
                } else {
                    left.max(right)
                })
            } else {
                let (Value::U64(left), Value::U64(right)) = (left, right) else {
                    return Err(());
                };
                match *op {
                    "+" => Value::U64(left.checked_add(right).ok_or(())?),
                    "-" => Value::U64(left.checked_sub(right).ok_or(())?),
                    "==" => Value::Bool(left == right),
                    "!=" => Value::Bool(left != right),
                    "<" => Value::Bool(left < right),
                    "<=" => Value::Bool(left <= right),
                    ">" => Value::Bool(left > right),
                    ">=" => Value::Bool(left >= right),
                    _ => unreachable!(),
                }
            }
        }
    })
}

fn interpret(statements: &[Stmt], states: &mut [u64; 2], context: Context) -> Result<(), ()> {
    for statement in statements {
        match statement {
            Stmt::Assign(index, expr) => match eval(expr, *states, context)? {
                Value::U64(value) => states[*index] = value,
                _ => return Err(()),
            },
            Stmt::Require(expr) => match eval(expr, *states, context)? {
                Value::Bool(true) => {}
                _ => return Err(()),
            },
            Stmt::When(condition, body) => match eval(condition, *states, context)? {
                Value::Bool(true) => interpret(body, states, context)?,
                Value::Bool(false) => {}
                _ => return Err(()),
            },
        }
    }
    Ok(())
}

fn fuzz_pruv(iterations: usize) {
    let mut rng = Rng(0x4f42_19a7_c311_890d);
    let edges = [0, 1, 2, 7, 11, 99, 100, 101, u64::MAX - 1, u64::MAX];
    let mut compiled = 0usize;
    let mut comparisons = 0usize;
    for iteration in 0..iterations {
        let count = 1 + rng.below(3);
        let program = statements(&mut rng, count, 2);
        let text = source(&program);
        let contract = match noid_pruv::compile(&text, &parameters()) {
            Ok(contract) => contract,
            Err(_) => continue,
        };
        compiled += 1;
        for _ in 0..12 {
            let initial = [edges[rng.below(edges.len())], rng.next()];
            let height = edges[rng.below(edges.len())];
            let context = Context {
                height,
                fee: edges[rng.below(edges.len())],
                payout: rng.next(),
                retained: rng.next(),
                input_amount: rng.next(),
                before_deadline: height < 100,
                terminal: rng.below(2) == 0,
                has_payout: rng.below(2) == 0,
                payout_owner: [rng.next(), rng.next(), rng.next(), rng.next()],
                input_creation_id: rng.next(),
                input_slot: rng.next() as u32,
                retained_slot: rng.next() as u32,
                payout_slot: rng.next() as u32,
            };
            let mut expected = initial;
            let expected = interpret(&program, &mut expected, context).map(|_| expected);
            let actual = native::execute(
                &contract.opening().program,
                native::pack_state(initial),
                context,
            )
            .map(native::unpack_state)
            .map_err(|_| ());
            comparisons += 1;
            if actual != expected {
                eprintln!("MISMATCH iteration={iteration}\n{text}\ninitial={initial:?}\ncontext={context:?}\nexpected={expected:?}\nactual={actual:?}");
                std::process::exit(1);
            }
        }
    }
    assert!(compiled > 0 && comparisons > 0, "vacuous differential run");
    println!("pruv_differential_ok iterations={iterations} compiled={compiled} comparisons={comparisons}");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("fuzz-pruv") => fuzz_pruv(
            args.get(2)
                .and_then(|value| value.parse().ok())
                .unwrap_or(2000),
        ),
        Some("native-rights") => native_rights(),
        Some("fuzz-relation") => fuzz_relation(
            args.get(2)
                .and_then(|value| value.parse().ok())
                .unwrap_or(8),
        ),
        _ => {
            eprintln!("usage: contract-rights-audit fuzz-pruv [iterations] | native-rights | fuzz-relation [pairs]");
            std::process::exit(2);
        }
    }
}

fn native_rights() {
    use noid_chain::{apply_tx, ApplyError, ChainState, SlotValue};
    use noid_gkr::{
        wallet_authorization::{
            prove_experimental_object_authorization, verify_experimental_object_authorization,
        },
        OwnerAuthWitness,
    };
    use noid_poseidon2b::primitives::{derive_address, SpendSecret};
    use noid_tx::experimental_object::{
        applications::{allowance_wallet, timelocked_vault},
        policy::{CLAIM_CONTINUE, RECOVERY_CLOSE},
        ObjectError, ObjectRules,
    };
    use noid_tx::{TxInput, TxOutput};

    let spending_secret = SpendSecret::from_bytes([1; 32]);
    let recovery_secret = SpendSecret::from_bytes([2; 32]);
    let attacker_secret = SpendSecret::from_bytes([9; 32]);
    let attacker = derive_address(&attacker_secret);
    let spender = derive_address(&spending_secret);
    let recovery = derive_address(&recovery_secret);
    let payee = Address([3; 32]);
    let object = allowance_wallet(spender, recovery, Some(payee), 50, 100, 500, 1);
    let coin = TxInput {
        slot_index: 1,
        amount: 10_000,
        creation_id: 7,
    };
    let payout = TxOutput {
        slot_index: 3,
        amount: 500,
        owner: payee,
    };
    let page = object
        .build_payment(coin, 2, 10, [4; 32], 49, payout)
        .unwrap();
    let checked = object.check_call(&page, 49).unwrap();
    assert_eq!(checked.authority, spender);
    assert_eq!(page.body.outputs[0].amount, 9_490);
    assert_eq!(
        page.body.inputs[0].amount,
        page.body.outputs[0].amount + page.body.outputs[1].amount + page.body.fee
    );

    let bundle = prove_experimental_object_authorization(
        &page,
        &object,
        49,
        OwnerAuthWitness::new(spending_secret),
    )
    .unwrap();
    verify_experimental_object_authorization(&page, &object, 49, &bundle).unwrap();
    assert!(prove_experimental_object_authorization(
        &page,
        &object,
        49,
        OwnerAuthWitness::new(attacker_secret),
    )
    .is_err());
    assert!(verify_experimental_object_authorization(&page, &object, 50, &bundle).is_err());
    let mut retargeted = page.clone();
    retargeted.body.outputs[1].owner = attacker;
    assert_eq!(
        object.check_call(&retargeted, 49),
        Err(ObjectError::Recipient)
    );
    assert!(verify_experimental_object_authorization(&retargeted, &object, 49, &bundle).is_err());
    assert!(object
        .build_payment(
            coin,
            2,
            10,
            [4; 32],
            49,
            TxOutput {
                amount: 501,
                ..payout
            }
        )
        .is_err());
    assert!(object.build_call(coin, 2, 10, [4; 32], 49, true).is_err());

    let mut state = ChainState::from_sparse_utxos(
        4,
        &[(
            1,
            SlotValue::with_owner_fields(10_000, 7, object.root().as_fields()),
        )],
        7,
    )
    .unwrap();
    let supply_before = state.circulating_supply_micronoid;
    apply_tx(&mut state, &page.body).unwrap();
    assert_eq!(state.circulating_supply_micronoid, supply_before - 10);
    assert_eq!(state.state.slot(1), SlotValue::EMPTY);
    assert_eq!(state.state.slot(2).amount(), 9_490);
    assert_eq!(state.state.slot(3).amount(), 500);
    let after_first = state.utxo_root;
    assert_eq!(
        apply_tx(&mut state, &page.body),
        Err(ApplyError::UnknownOrSpentInput)
    );
    assert_eq!(state.utxo_root, after_first);
    assert_eq!(state.circulating_supply_micronoid, supply_before - 10);

    let successor = object.successor(checked.next);
    let successor_coin = TxInput {
        slot_index: 2,
        amount: state.state.slot(2).amount(),
        creation_id: state.state.slot(2).creation_id(),
    };
    let second = successor
        .build_payment(
            successor_coin,
            4,
            10,
            [4; 32],
            49,
            TxOutput {
                slot_index: 5,
                ..payout
            },
        )
        .unwrap();
    apply_tx(&mut state, &second.body).unwrap();
    assert_eq!(state.circulating_supply_micronoid, supply_before - 20);

    let vault = timelocked_vault(spender, 50, 100);
    for terminal in [false, true] {
        assert_eq!(
            vault.build_call(coin, 2, 10, [4; 32], 49, terminal),
            Err(ObjectError::Policy)
        );
    }
    assert!(vault.build_call(coin, 2, 10, [4; 32], 50, true).is_ok());

    let mut once = allowance_wallet(spender, recovery, Some(payee), 50, 100, 500, 1);
    once.rules = ObjectRules {
        max_fee: 100,
        min_retained: 1,
        max_payout: 500,
        modes: CLAIM_CONTINUE | RECOVERY_CLOSE,
    };
    once.program[0] = native::Instruction::new(
        native::Opcode::AssertEqual,
        native::Register::State0,
        native::Operand::State0,
        native::Operand::Zero,
        0,
    )
    .to_fields();
    once.program[1] = native::Instruction::new(
        native::Opcode::Add,
        native::Register::State0,
        native::Operand::State0,
        native::Operand::One,
        0,
    )
    .to_fields();
    let first = once
        .build_payment(coin, 2, 10, [4; 32], 49, payout)
        .unwrap();
    let first_next = once.check_call(&first, 49).unwrap().next;
    assert_eq!(native::unpack_state(first_next)[0], 1);
    assert!(once.check_call(&first, 49).is_ok());
    let once_successor = once.successor(first_next);
    assert!(once_successor
        .build_payment(
            TxInput {
                slot_index: 2,
                amount: 9_490,
                creation_id: 8,
            },
            4,
            10,
            [4; 32],
            49,
            TxOutput {
                slot_index: 5,
                ..payout
            }
        )
        .is_err());
    println!("native_rights_ok wrong_authority_rejected recipient_retarget_rejected allowance_cap_enforced vault_lock_enforced");
    println!("state_transition_ok conservation=input-fee stale_replay=UnknownOrSpentInput replay_atomic=true successor_creation_id={}", successor_coin.creation_id);
    println!("counter_control_ok first_call_accepted successor_repeat_rejected stateless_preflight_replay_requires_state=true");
}

fn relation_run(
    program: &native::Program,
    current: noid_core::Block128,
    next: noid_core::Block128,
    context: Context,
    compact: bool,
) -> bool {
    use noid_recursive::acceptance::trace::{
        alloc_block, integer_program, FieldR1csBuilder, LinExpr, F128,
    };
    let mut b = FieldR1csBuilder::new();
    let mut field = |value| alloc_block(&mut b, noid_core::Block128(value));
    let program = program.map(|pair| pair.map(|value| field(value.0)));
    let current = field(current.0);
    let next = field(next.0);
    let context = integer_program::ContextTrace {
        height: field(context.height as u128),
        fee: field(context.fee as u128),
        payout: field(context.payout as u128),
        retained: field(context.retained as u128),
        input_amount: field(context.input_amount as u128),
        before_deadline: field(context.before_deadline as u128),
        terminal: field(context.terminal as u128),
        has_payout: field(context.has_payout as u128),
        payout_owner: context.payout_owner.map(|value| field(value as u128)),
        input_creation_id: field(context.input_creation_id as u128),
        input_slot: field(context.input_slot as u128),
        retained_slot: field(context.retained_slot as u128),
        payout_slot: field(context.payout_slot as u128),
    };
    if compact {
        integer_program::bind_program_v2_1(
            &mut b,
            &program,
            &current,
            &next,
            &context,
            &LinExpr::constant(F128::ONE),
        );
    } else {
        integer_program::bind_program(
            &mut b,
            &program,
            &current,
            &next,
            &context,
            &LinExpr::constant(F128::ONE),
        );
    }
    let (matrix, witness) = b.build();
    matrix.satisfies(&witness)
}

fn fuzz_relation(pairs: usize) {
    let mut rng = Rng(0x9b17_55aa_23dd_0911);
    let edges = [0, 1, 2, u64::MAX - 1, u64::MAX, 1 << 63];
    let mut comparisons = 0usize;
    for compact in [false, true] {
        for opcode in native::Opcode::ALL {
            for index in 0..pairs {
                let left = if index < edges.len() {
                    edges[index]
                } else {
                    rng.next()
                };
                let right = if index % 2 == 0 {
                    edges[(index + 1) % edges.len()]
                } else {
                    rng.next()
                };
                let mut program = native::EMPTY_PROGRAM;
                program[0] = native::Instruction::new(
                    opcode,
                    native::Register::State0,
                    native::Operand::State0,
                    native::Operand::Immediate,
                    right,
                )
                .to_fields();
                let current = native::pack_state([left, 77]);
                let expected = native::execute(&program, current, Context::default());
                let next = expected.unwrap_or(current);
                let valid = relation_run(&program, current, next, Context::default(), compact);
                assert_eq!(
                    valid,
                    expected.is_ok(),
                    "compact={compact} {opcode:?}({left},{right})"
                );
                if let Ok(next) = expected {
                    assert!(!relation_run(
                        &program,
                        current,
                        noid_core::Block128(next.0 ^ 1),
                        Context::default(),
                        compact,
                    ));
                    comparisons += 1;
                }
                comparisons += 1;
            }
        }
    }
    assert!(comparisons > 0, "vacuous relation run");
    println!("relation_differential_ok pairs_per_opcode={pairs} comparisons={comparisons} legacy_and_v2_1=true");
}
