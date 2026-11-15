//! Wallet/secret-handling audit harness against the pinned parano1d tree.
//!
//! `wallet_safety_repro::wallet` is the node's real `noid_node::wallet`
//! module compiled in via #[path]; every intent below is built by the
//! production builder and authorized by the production GKR prover.

use noid_gkr::{
    prove_paged_spend_authorization, verify_paged_spend_authorization, OwnerAuthWitness,
    WalletAuthorizationBundle,
};
use noid_poseidon2b::primitives::{derive_address, Address, SpendSecret};
use noid_rpc::types::WalletSendResult;
use noid_tx::{
    output_bitmap_bit, validate_paged_spend, OwnerAuthorizationVersion, PagedSpendIntent, TxBody,
    TxInput, TxOutput, TxPage, PAGED_SPEND_END_BIT, PAGED_SPEND_START_BIT, TX_INPUTS, TX_OUTPUTS,
};
use wallet_safety_repro::wallet::{builder, state};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

const PAYMENT: u64 = 90;
const ALTERED_PAYMENT: u64 = 7;
const FEE: u64 = 5;

fn synthetic_secret() -> Zeroizing<[u8; 32]> {
    Zeroizing::new(std::array::from_fn(|index| {
        0x51u8.wrapping_add((index as u8).wrapping_mul(13))
    }))
}

/// Wallet fixture mirroring the node's own builder tests: one funded UTXO of
/// 100 micronoid at slot 41, creation 9, on the wallet's active address.
fn funded_wallet() -> (tempfile::TempDir, state::WalletState) {
    let directory = tempfile::tempdir().unwrap();
    let key = directory.path().join("wallet.key");
    let mut wallet = state::WalletState::create_or_load(key).unwrap();
    let owner = wallet.active_address();
    wallet.utxos.insert(
        41,
        state::WalletUtxo {
            slot_index: 41,
            value: 100,
            creation_id: 9,
            address: owner,
            key_index: 0,
            confirmed_height: 1,
        },
    );
    (directory, wallet)
}

/// Run one payment through the exact production build path used by
/// `WalletHandle::build_send` (mod.rs:1416-1461): extract_build_data on the
/// live wallet state, then build_and_prove_tx_for_version. V4 is the post-
/// activation authorization version; the shape gate and the WalletSendResult
/// contract under test are version-independent.
fn production_intent(
    wallet: &mut state::WalletState,
    recipient: [u8; 32],
    payment: u64,
) -> PagedSpendIntent {
    let build = builder::extract_build_data(
        wallet,
        payment,
        FEE,
        [0xA5; 32],
        vec![1_001, 1_002],
        24,
        &std::collections::HashSet::new(),
    )
    .unwrap();
    let (_, bytes) = builder::build_and_prove_tx_for_version(
        recipient,
        payment,
        FEE,
        build,
        OwnerAuthorizationVersion::V4,
    )
    .unwrap();
    PagedSpendIntent::from_bytes(&bytes).unwrap()
}

fn hand_rolled_intent(
    secret_bytes: &[u8; 32],
    recipient: [u8; 32],
    payment: u64,
) -> PagedSpendIntent {
    let secret = SpendSecret::from_bytes(*secret_bytes);
    let owner = derive_address(&secret);
    let mut inputs = [TxInput::dummy(); TX_INPUTS];
    inputs[0] = TxInput {
        slot_index: 41,
        amount: 100,
        creation_id: 9,
    };
    let mut outputs = [TxOutput::dummy(); TX_OUTPUTS];
    outputs[0] = TxOutput {
        slot_index: 1_001,
        amount: payment,
        owner: Address(recipient),
    };
    outputs[1] = TxOutput {
        slot_index: 1_002,
        amount: 100 - payment - FEE,
        owner,
    };
    let page = TxPage::new(TxBody {
        epoch_anchor: [0xA5; 32],
        fee: FEE,
        input_owner: owner,
        inputs,
        outputs,
        validity_bitmap: 1
            | output_bitmap_bit(0)
            | output_bitmap_bit(1)
            | PAGED_SPEND_START_BIT
            | PAGED_SPEND_END_BIT,
        is_coinbase: false,
    })
    .unwrap();
    let bundle =
        prove_paged_spend_authorization(std::slice::from_ref(&page), OwnerAuthWitness::new(secret))
            .unwrap();
    PagedSpendIntent::new(vec![page], bundle.to_bytes().unwrap()).unwrap()
}

fn verify(intent: &PagedSpendIntent) -> Result<(), noid_gkr::VerifyAuthorizationError> {
    let bundle = WalletAuthorizationBundle::from_bytes(&intent.authorization_bytes).unwrap();
    verify_paged_spend_authorization(&intent.pages, &bundle)
}

/// The complete post-build divergence condition from
/// noid_rpc/src/server.rs:1164-1167, evaluated on validate_paged_spend facts.
fn server_gate_rejects(
    intent: &PagedSpendIntent,
    expected_inputs: usize,
    expected_outputs: usize,
) -> bool {
    let facts = validate_paged_spend(&intent.pages).unwrap();
    facts.fee != FEE
        || usize::from(facts.live_inputs) != expected_inputs
        || usize::from(facts.live_outputs) != expected_outputs
}

#[test]
fn authorization_binds_outputs_and_wire_bytes_omit_the_synthetic_secret() {
    let secret = synthetic_secret();
    let requested = hand_rolled_intent(&secret, [0xB0; 32], PAYMENT);
    let wire = requested.to_bytes().unwrap();
    assert!(!wire
        .windows(secret.len())
        .any(|window| window == &secret[..]));
    verify(&requested).unwrap();

    let mut changed = requested.pages[0].body.clone();
    changed.outputs[0].owner = Address([0xC0; 32]);
    let changed = PagedSpendIntent {
        pages: vec![TxPage::new(changed).unwrap()],
        authorization_bytes: requested.authorization_bytes.clone(),
    };
    assert!(verify(&changed).is_err());
}

/// CONTROL (honest node): the production builder passes the request straight
/// into the signed page, so the reviewed/displayed values equal the signed
/// values and the server's shape gate passes.
#[test]
fn production_builder_signs_exactly_the_requested_payment() {
    let (_dir, mut wallet) = funded_wallet();
    let requested_recipient = [0xB0; 32];
    let intent = production_intent(&mut wallet, requested_recipient, PAYMENT);

    verify(&intent).unwrap();
    assert!(!server_gate_rejects(&intent, 1, 2));

    let signed_output = intent.pages[0].body.outputs[0];
    assert_eq!(signed_output.owner.0, requested_recipient);
    assert_eq!(signed_output.amount, PAYMENT);
    let change = intent.pages[0].body.outputs[1];
    assert_eq!(change.amount, 100 - PAYMENT - FEE);
    assert_eq!(change.owner, wallet.active_address());
}

/// GATE BLINDNESS (defense-in-depth gap, no divergent production path found):
/// two intents built by the production builder from the same wallet with the
/// same fee and the same live input/output counts but DIFFERENT recipient and
/// amount are both indistinguishable to the server.rs:1164-1167 gate, to
/// WalletSendResult (no recipient field; amount echoes the request at
/// server.rs:1202-1208), and to the GUI success screen (backend.rs:820-827
/// renders the request's recipient string). Only the txid differs.
#[test]
fn divergence_gate_and_result_cannot_detect_recipient_or_amount_substitution() {
    let (_dir, mut wallet) = funded_wallet();
    let requested_recipient = [0xB0; 32];
    let altered_recipient = [0xC0; 32];

    let reviewed = production_intent(&mut wallet, requested_recipient, PAYMENT);
    let altered = production_intent(&mut wallet, altered_recipient, ALTERED_PAYMENT);
    verify(&reviewed).unwrap();
    verify(&altered).unwrap();

    assert!(!server_gate_rejects(&reviewed, 1, 2));
    assert!(!server_gate_rejects(&altered, 1, 2));

    let signed_output = altered.pages[0].body.outputs[0];
    assert_eq!(signed_output.owner.0, altered_recipient);
    assert_eq!(signed_output.amount, ALTERED_PAYMENT);

    // The wire result a GUI client receives cannot express the substitution:
    // it has no recipient field and its amount is the echoed request value.
    let result_for_altered = WalletSendResult {
        txid: hex::encode(altered.logical_txid().0),
        amount_micronoid: PAYMENT,
        fee_micronoid: FEE,
        input_count: 1,
        output_count: 2,
    };
    let encoded = serde_json::to_value(&result_for_altered).unwrap();
    assert!(encoded.get("recipient").is_none());
    assert!(encoded.get("intent").is_none());
    assert_eq!(encoded["amount_micronoid"], serde_json::json!(PAYMENT));
    assert_eq!(
        encoded["txid"],
        serde_json::json!(hex::encode(altered.logical_txid().0))
    );
    assert_ne!(
        encoded["txid"],
        serde_json::json!(hex::encode(reviewed.logical_txid().0))
    );

    // What the GUI success screen shows (backend.rs:820-827): the request's
    // recipient and this echoed amount — not values recovered from the signed
    // intent.
    let displayed_recipient = requested_recipient;
    let displayed_amount = PAYMENT;
    assert_ne!(displayed_recipient, signed_output.owner.0);
    assert_ne!(displayed_amount, signed_output.amount);
}

#[test]
fn secret_owners_stay_opaque_redacted_and_zeroizing() {
    static_assertions::assert_not_impl_any!(
        SpendSecret: Copy,
        Clone,
        PartialEq,
        Eq,
        std::hash::Hash,
        serde::Serialize,
        serde::de::DeserializeOwned
    );
    static_assertions::assert_not_impl_any!(
        OwnerAuthWitness: Copy,
        Clone,
        std::fmt::Debug,
        serde::Serialize,
        serde::de::DeserializeOwned
    );
    fn assert_zeroized<T: Zeroize + ZeroizeOnDrop>() {}
    assert_zeroized::<SpendSecret>();
    assert_zeroized::<OwnerAuthWitness>();

    let secret = SpendSecret::from_bytes([0xA7; 32]);
    assert_eq!(format!("{secret:?}"), "SpendSecret([REDACTED])");
}

/// LIBRARY-LIFETIME FOOTGUN (no production path found — see README): a
/// WalletState loaded before an on-disk master-secret replacement keeps the
/// OLD secret (it can still sign) and its later writes land in the NEW
/// wallet's history journal.
#[test]
fn import_does_not_revoke_a_loaded_wallet_or_isolate_replacement_artifacts() {
    let directory = tempfile::tempdir().unwrap();
    let key = directory.path().join("wallet.key");
    let first = "11".repeat(32);
    let second = "22".repeat(32);
    state::import_generated_master_secret(&key, &first).unwrap();

    let mut stale = state::WalletState::create_or_load(key.clone()).unwrap();
    let stale_owner = stale.active_address();
    stale.utxos.insert(
        41,
        state::WalletUtxo {
            slot_index: 41,
            value: 100,
            creation_id: 9,
            address: stale_owner,
            key_index: 0,
            confirmed_height: 1,
        },
    );

    state::import_generated_master_secret(&key, &second).unwrap();
    let replacement = state::WalletState::create_or_load(key.clone()).unwrap();
    assert_ne!(replacement.active_address(), stale_owner);

    let build = builder::extract_build_data(
        &stale,
        PAYMENT,
        FEE,
        [0xA5; 32],
        vec![1_001, 1_002],
        24,
        &std::collections::HashSet::new(),
    )
    .unwrap();
    let (_, bytes) = builder::build_and_prove_tx_for_version(
        [0xB0; 32],
        PAYMENT,
        FEE,
        build,
        OwnerAuthorizationVersion::V4,
    )
    .unwrap();
    let intent = PagedSpendIntent::from_bytes(&bytes).unwrap();
    assert_eq!(intent.pages[0].body.input_owner, stale_owner);
    verify(&intent).unwrap();

    stale
        .record_pending_send([0xD1; 32], PAYMENT, [0xB0; 32])
        .unwrap();
    let contaminated = state::WalletState::create_or_load(key).unwrap();
    assert_eq!(contaminated.active_address(), replacement.active_address());
    assert_eq!(contaminated.history.len(), 1);
    assert_eq!(contaminated.history[0].own_key_index, Some(0));
    assert_eq!(
        contaminated.history[0].own_address.as_deref(),
        Some(stale_owner.to_bech32().as_str())
    );
    assert_ne!(
        contaminated.history[0].own_address.as_deref(),
        Some(contaminated.active_address().to_bech32().as_str())
    );
}
