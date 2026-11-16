// Production wallet builder and authorization verifier, local synthetic state.
use noid_gkr::{verify_paged_spend_authorization, WalletAuthorizationBundle};
use noid_poseidon2b::primitives::Address;
use noid_tx::{validate_paged_spend, OwnerAuthorizationVersion, PagedSpendIntent};
use wallet_safety_repro::wallet::{builder, state};

fn main() {
    let attacker_text = "o1uqppny90hhl7e5gyvnxgh2dj60gddehxr7l97ue04au60qfszxdsmk8x3t";
    let attacker = Address::parse(attacker_text).unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let key = temporary.path().join("payer.key");
    let mut payer = state::WalletState::create_or_load(key).unwrap();
    let owner = payer.active_address();
    payer.utxos.insert(41, state::WalletUtxo {
        slot_index: 41,
        value: 100,
        creation_id: 9,
        address: owner,
        key_index: 0,
        confirmed_height: 1,
    });
    let build = builder::extract_build_data(
        &mut payer, 90, 5, [0xA5; 32], vec![1_001, 1_002], 24,
        &std::collections::HashSet::new(),
    ).unwrap();
    let (_, wire) = builder::build_and_prove_tx_for_version(
        attacker.0, 90, 5, build, OwnerAuthorizationVersion::V4,
    ).unwrap();
    let intent = PagedSpendIntent::from_bytes(&wire).unwrap();
    let facts = validate_paged_spend(&intent.pages).unwrap();
    let auth = WalletAuthorizationBundle::from_bytes(&intent.authorization_bytes).unwrap();
    verify_paged_spend_authorization(&intent.pages, &auth).unwrap();
    let output = intent.pages[0].body.outputs[0];
    assert_eq!(output.owner.0, attacker.0);
    assert_eq!(output.amount, 90);
    assert_eq!(facts.fee, 5);
    println!("signed_destination={}", output.owner.to_bech32());
    println!("signed_amount={} fee={} auth=valid", output.amount, facts.fee);
    println!("PASS: production wallet builder signs a payment to the substituted receive address");
}
