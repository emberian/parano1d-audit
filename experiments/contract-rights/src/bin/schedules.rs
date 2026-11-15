use noid_poseidon2b::primitives::Address;
use noid_tx::{
    experimental_object::{
        applications::{
            period_budget_wallet, recurring_payment, tranche_vesting, PeriodBudget,
            RecurringPayment, TrancheVesting,
        },
        integer_program::unpack_state,
        ObjectError, ObjectOpening,
    },
    TxInput, TxOutput, TxPage,
};

fn input() -> TxInput {
    TxInput {
        slot_index: 1,
        amount: 10_000,
        creation_id: 3,
    }
}

fn payment(
    object: &ObjectOpening,
    input: TxInput,
    height: u64,
    fee: u64,
    amount: u64,
) -> Result<TxPage, ObjectError> {
    object.build_payment(
        input,
        input.slot_index + 3,
        fee,
        [4; 32],
        height,
        TxOutput {
            slot_index: input.slot_index + 4,
            amount,
            owner: object.claim_recipient,
        },
    )
}

fn successor(object: &ObjectOpening, page: &TxPage, height: u64) -> (ObjectOpening, TxInput) {
    let checked = object.check_call(page, height).unwrap();
    assert_eq!(
        page.body.inputs[0].amount,
        page.body.fee + page.body.outputs[0].amount + page.body.outputs[1].amount
    );
    (
        object.successor(checked.next),
        TxInput {
            slot_index: page.body.outputs[0].slot_index,
            amount: page.body.outputs[0].amount,
            creation_id: page.body.inputs[0].creation_id + 2,
        },
    )
}

fn main() {
    let spender = Address([1; 32]);
    let recovery = Address([2; 32]);
    let payee = Address([3; 32]);

    let (mut budget, mut coin) = (
        period_budget_wallet(
            spender,
            recovery,
            Some(payee),
            PeriodBudget {
                start_height: 100,
                period_blocks: 10,
                budget: 100,
                recover_at: 150,
                max_fee: 10,
                max_payout: 100,
                min_retained: 20,
            },
        )
        .unwrap(),
        input(),
    );
    assert!(payment(&budget, coin, 99, 5, 55).is_err());
    let page = payment(&budget, coin, 100, 5, 55).unwrap();
    (budget, coin) = successor(&budget, &page, 100);
    assert_eq!(unpack_state(budget.state), [40, 110]);
    assert!(payment(&budget, coin, 109, 0, 41).is_err());
    let page = payment(&budget, coin, 110, 10, 90).unwrap();
    (budget, coin) = successor(&budget, &page, 110);
    assert_eq!(unpack_state(budget.state), [0, 120]);
    assert!(payment(&budget, coin, 119, 0, 1).is_err());
    let close = budget
        .build_call(coin, coin.slot_index + 3, 10, [4; 32], 150, true)
        .unwrap();
    assert_eq!(close.body.outputs[0].owner, recovery);
    println!("period_budget_ok early_rejected fee_included reset_at_110 exhausted_window_rejected recovery_recipient_bound");

    let (mut recurring, mut coin) = (
        recurring_payment(
            recovery,
            payee,
            RecurringPayment {
                first_due_height: 100,
                period_blocks: 10,
                payment: 50,
                recover_at: 180,
                max_fee: 5,
            },
        )
        .unwrap(),
        input(),
    );
    assert!(payment(&recurring, coin, 99, 5, 50).is_err());
    for amount in [49, 51] {
        assert!(payment(&recurring, coin, 100, 5, amount).is_err());
    }
    let page = payment(&recurring, coin, 101, 5, 50).unwrap();
    (recurring, coin) = successor(&recurring, &page, 101);
    assert_eq!(unpack_state(recurring.state), [1, 111]);
    assert!(payment(&recurring, coin, 110, 5, 50).is_err());
    let page = payment(&recurring, coin, 160, 5, 50).unwrap();
    (recurring, coin) = successor(&recurring, &page, 160);
    assert_eq!(unpack_state(recurring.state), [2, 170]);
    let close = recurring
        .build_call(coin, coin.slot_index + 3, 5, [4; 32], 180, true)
        .unwrap();
    assert_eq!(close.body.outputs[0].owner, recovery);
    println!("recurring_payment_ok due_height_and_exact_amount_enforced inclusion_relative_schedule missed_periods_not_accumulated payer_recovery_bound");

    let (mut vesting, mut coin) = (
        tranche_vesting(
            payee,
            TrancheVesting {
                first_unlock_height: 100,
                period_blocks: 10,
                tranche_amount: 50,
                mature_at: 150,
                max_fee: 5,
            },
        )
        .unwrap(),
        input(),
    );
    assert!(payment(&vesting, coin, 99, 5, 50).is_err());
    for count in 1..=3 {
        let page = payment(&vesting, coin, 129, 5, 50).unwrap();
        (vesting, coin) = successor(&vesting, &page, 129);
        assert_eq!(unpack_state(vesting.state), [count * 50, 100 + count * 10]);
    }
    assert!(payment(&vesting, coin, 129, 5, 50).is_err());
    let close = vesting
        .build_call(coin, coin.slot_index + 3, 5, [4; 32], 150, true)
        .unwrap();
    assert_eq!(close.body.outputs[0].owner, payee);
    println!("tranche_vesting_ok early_rejected missed_tranches_one_per_call fourth_premature_rejected maturity_recipient_bound");
}
