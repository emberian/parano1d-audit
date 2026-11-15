use noid_gkr::wallet_authorization::{
    prove_paged_spend_authorization_for_version, verify_paged_spend_authorization,
    verify_paged_spend_authorization_for_version, OwnerAuthWitness, WalletAuthorizationBundle,
};
use noid_poseidon2b::primitives::{derive_address, Address, SpendSecret};
use noid_tx::{
    output_bitmap_bit, OwnerAuthorizationVersion, TxInput, TxOutput, TxPage, PAGED_SPEND_END_BIT,
    PAGED_SPEND_START_BIT, TX_INPUTS, TX_OUTPUTS,
};
use serde_json::json;

fn fixture() -> (Vec<TxPage>, [u8; 32]) {
    let mut secret_bytes = [0u8; 32];
    for (index, byte) in secret_bytes.iter_mut().enumerate() {
        *byte = 7u8
            .wrapping_mul(31)
            .wrapping_add(index as u8)
            .wrapping_add(11);
    }
    let secret = SpendSecret::from_bytes(secret_bytes);
    let mut inputs = [TxInput::dummy(); TX_INPUTS];
    inputs[0] = TxInput {
        slot_index: 20,
        amount: 100,
        creation_id: 3,
    };
    let mut outputs = [TxOutput::dummy(); TX_OUTPUTS];
    outputs[0] = TxOutput {
        slot_index: 32,
        amount: 95,
        owner: Address([0xb0; 32]),
    };
    let body = noid_tx::TxBody {
        epoch_anchor: [0xa5; 32],
        fee: 5,
        input_owner: derive_address(&secret),
        inputs,
        outputs,
        validity_bitmap: 1 | output_bitmap_bit(0) | PAGED_SPEND_START_BIT | PAGED_SPEND_END_BIT,
        is_coinbase: false,
    };
    (
        vec![TxPage::new(body).expect("canonical page")],
        secret_bytes,
    )
}

pub(crate) fn run() -> Result<(), Box<dyn std::error::Error>> {
    let (pages, secret_bytes) = fixture();
    let make = || OwnerAuthWitness::new(SpendSecret::from_bytes(secret_bytes));
    let v3 =
        prove_paged_spend_authorization_for_version(&pages, make(), OwnerAuthorizationVersion::V3)?;
    let v4 =
        prove_paged_spend_authorization_for_version(&pages, make(), OwnerAuthorizationVersion::V4)?;

    let v3_as_v3 =
        verify_paged_spend_authorization_for_version(&pages, &v3, OwnerAuthorizationVersion::V3);
    let v3_as_v4 =
        verify_paged_spend_authorization_for_version(&pages, &v3, OwnerAuthorizationVersion::V4);
    let v4_as_v3 =
        verify_paged_spend_authorization_for_version(&pages, &v4, OwnerAuthorizationVersion::V3);
    let v4_as_v4 =
        verify_paged_spend_authorization_for_version(&pages, &v4, OwnerAuthorizationVersion::V4);
    let v3_default = verify_paged_spend_authorization(&pages, &v3);
    let v4_default = verify_paged_spend_authorization(&pages, &v4);

    let v3_bytes = v3.to_bytes()?;
    let v4_bytes = v4.to_bytes()?;
    let decoded_v3 = WalletAuthorizationBundle::from_bytes(&v3_bytes)?;
    let decoded_v4 = WalletAuthorizationBundle::from_bytes(&v4_bytes)?;
    let decoded_v3_as_v4 = verify_paged_spend_authorization_for_version(
        &pages,
        &decoded_v3,
        OwnerAuthorizationVersion::V4,
    );
    let decoded_v4_as_v3 = verify_paged_spend_authorization_for_version(
        &pages,
        &decoded_v4,
        OwnerAuthorizationVersion::V3,
    );

    assert!(v3_as_v3.is_ok());
    assert!(v3_as_v4.is_err());
    assert!(v4_as_v3.is_err());
    assert!(v4_as_v4.is_ok());
    assert!(v3_default.is_err());
    assert!(v4_default.is_ok());
    assert!(decoded_v3_as_v4.is_err());
    assert!(decoded_v4_as_v3.is_err());
    assert_ne!(v3_bytes, v4_bytes);

    let output = json!({
        "check": "owner-authorization-version-separation",
        "v3_bytes": v3_bytes.len(),
        "v4_bytes": v4_bytes.len(),
        "bundle_prefix_v3": crate::hex(&v3_bytes[..12]),
        "bundle_prefix_v4": crate::hex(&v4_bytes[..12]),
        "v3_as_v3": v3_as_v3.is_ok(),
        "v3_as_v4": v3_as_v4.is_ok(),
        "v3_as_v4_error": format!("{:?}", v3_as_v4.unwrap_err()),
        "v4_as_v3": v4_as_v3.is_ok(),
        "v4_as_v3_error": format!("{:?}", v4_as_v3.unwrap_err()),
        "v4_as_v4": v4_as_v4.is_ok(),
        "v3_default_v4_api": v3_default.is_ok(),
        "v4_default_v4_api": v4_default.is_ok(),
        "decoded_v3_as_v4": decoded_v3_as_v4.is_ok(),
        "decoded_v4_as_v3": decoded_v4_as_v3.is_ok(),
    });
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}
