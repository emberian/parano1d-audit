use noid_gkr::wallet_authorization::{
    prove_wallet_authorization, verify_wallet_authorization, OwnerAuthWitness,
};
use noid_poseidon2b::primitives::{derive_address, Address, SpendSecret, TxBodyHash};
use noid_tx::{output_bitmap_bit, TxBody, TxInput, TxOutput, TX_INPUTS, TX_OUTPUTS};

fn body(secret: &SpendSecret, epoch_salt: u8) -> TxBody {
    let owner = derive_address(secret);
    let mut inputs = [TxInput::dummy(); TX_INPUTS];
    inputs[0] = TxInput {
        slot_index: 0x101,
        amount: 25,
        creation_id: 0x202,
    };
    let mut outputs = [TxOutput::dummy(); TX_OUTPUTS];
    outputs[0] = TxOutput {
        slot_index: 0x303,
        amount: 20,
        owner: Address([0x44; 32]),
    };
    let mut epoch_anchor = [0x55; 32];
    epoch_anchor[0] ^= epoch_salt;
    TxBody {
        epoch_anchor,
        fee: 5,
        input_owner: owner,
        inputs,
        outputs,
        validity_bitmap: 1 | output_bitmap_bit(0),
        is_coinbase: false,
    }
}

fn hash_hex(hash: TxBodyHash) -> String {
    hash.0.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn main() {
    let secret = SpendSecret::from_bytes([0x31; 32]);
    let original = body(&secret, 0);
    let changed_context = body(&secret, 1);
    assert_ne!(original.txid(), changed_context.txid());

    let first_body = original.clone();
    let second_body = original.clone();
    let first = std::thread::spawn(move || {
        prove_wallet_authorization(
            &first_body,
            OwnerAuthWitness::new(SpendSecret::from_bytes([0x31; 32])),
        )
        .expect("first concurrent proof")
    });
    let second = std::thread::spawn(move || {
        prove_wallet_authorization(
            &second_body,
            OwnerAuthWitness::new(SpendSecret::from_bytes([0x31; 32])),
        )
        .expect("second concurrent proof")
    });
    let first = first.join().expect("first prover thread");
    let second = second.join().expect("second prover thread");

    verify_wallet_authorization(&original, &first).expect("first proof in session A");
    verify_wallet_authorization(&original, &second).expect("second proof in session A");
    verify_wallet_authorization(&original, &first).expect("first proof replayed in session B");
    verify_wallet_authorization(&original, &second).expect("second proof replayed in session B");

    let first_changed = verify_wallet_authorization(&changed_context, &first);
    let second_changed = verify_wallet_authorization(&changed_context, &second);
    assert!(first_changed.is_err());
    assert!(second_changed.is_err());

    let first_bytes = first.to_bytes().expect("first wire encoding");
    let second_bytes = second.to_bytes().expect("second wire encoding");
    println!("statement={}", hash_hex(original.txid()));
    println!("changed_statement={}", hash_hex(changed_context.txid()));
    println!("proofs_have_distinct_bytes={}", first_bytes != second_bytes);
    println!(
        "proofs_have_distinct_source_commitments={}",
        first.proof.source_commitment != second.proof.source_commitment
    );
    println!("both_proofs_accept_in_two_sessions=true");
    println!("both_proofs_reject_changed_context=true");
    println!("first_proof_bytes={}", first_bytes.len());
    println!("second_proof_bytes={}", second_bytes.len());
}
