use noid_poseidon2b::primitives::Address;
use noid_pruv::native::{NativeTemplate, Owner};
use noid_tx::experimental_object::{applications, ObjectOpening};
use noid_tx::native_assets::launch::LAUNCH_MAX_SUPPLY;
use serde_json::json;

fn main() {
    let zero = Address([0; 32]);
    let ordinary = Address([1; 32]);
    let zero_text = zero.to_bech32();
    let ordinary_text = ordinary.to_bech32();

    // Address syntax itself admits zero, while Pruv's typed Owner does not.
    assert_eq!(Address::parse(&zero_text).unwrap(), zero);
    assert!(Owner::try_from(zero_text.clone()).is_err());
    assert!(Owner::try_from(ordinary_text.clone()).is_ok());

    // The same ObjectOpening validation is used by createObject's descriptor path.
    // These are synthetically constructed openings, with no funds or live chain.
    let both_zero = applications::refundable_payment(zero, zero, 100, 1);
    assert!(both_zero.validate().is_ok());
    let encoded = both_zero.to_bytes().unwrap();
    assert_eq!(ObjectOpening::from_bytes(&encoded).unwrap(), both_zero);
    assert_eq!(both_zero.claim_authority, zero);
    assert_eq!(both_zero.recovery_recipient, zero);
    let ordinary_object = applications::refundable_payment(ordinary, ordinary, 100, 1);
    assert!(ordinary_object.validate().is_ok());
    println!("object opening: zero and ordinary policy addresses accepted; zero opening round-trips through wire parser");
    println!("address syntax: zero accepted; pruv Owner: zero rejected");

    let asset_id = "11".repeat(32);
    let transfer_zero = json!({
        "action": "transfer_token", "asset_id": asset_id,
        "units": "1", "recipient": zero_text
    });
    assert!(NativeTemplate::parse(&transfer_zero.to_string()).is_err());
    let transfer_ordinary = json!({
        "action": "transfer_token", "asset_id": "11".repeat(32),
        "units": "1", "recipient": ordinary_text
    });
    assert!(NativeTemplate::parse(&transfer_ordinary.to_string()).is_ok());
    println!("native transfer: zero recipient rejected; ordinary accepted");

    // Pruv performs static template validation; the RPC planner applies the
    // height-relative window. This control shows the static side only.
    let trade = json!({
        "action": "trade", "asset_id": "11".repeat(32),
        "buy": true, "gross_input": "1", "minimum_output": "1",
        "valid_until": 1_000_000_000u64
    });
    assert!(NativeTemplate::parse(&trade.to_string()).is_ok());
    let zero_expiry = json!({
        "action": "trade", "asset_id": "11".repeat(32),
        "buy": true, "gross_input": "1", "minimum_output": "1",
        "valid_until": 0
    });
    assert!(NativeTemplate::parse(&zero_expiry.to_string()).is_err());
    println!("native trade: distant positive expiry accepted; zero rejected");

    // The launch ceiling describes initial supply, not a static per-operation
    // range; the planner selects actual held balances and validates the output.
    let beyond_launch_supply = LAUNCH_MAX_SUPPLY.checked_add(1).unwrap();
    let transfer_large = json!({
        "action": "transfer_token", "asset_id": "11".repeat(32),
        "units": beyond_launch_supply.to_string(), "recipient": ordinary_text
    });
    let burn_large = json!({
        "action": "burn_token", "asset_id": "11".repeat(32),
        "units": beyond_launch_supply.to_string()
    });
    assert!(NativeTemplate::parse(&transfer_large.to_string()).is_ok());
    assert!(NativeTemplate::parse(&burn_large.to_string()).is_ok());
    println!("native transfer/burn: unit amount above launch ceiling accepted by static template");
}
