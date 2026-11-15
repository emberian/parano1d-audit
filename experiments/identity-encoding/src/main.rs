use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Instant,
};

use libp2p::PeerId;
use noid_chain::{
    block::{native_assets::validate_scheduled_block_page_stream, Block},
    block_header::semantic_header_id,
    consensus::{
        checks::validate_block_slot_conflicts, genesis_header, pow::block_id,
        tx_epoch_anchor_height_for_child,
    },
    fri_state::SlotValue,
    history_step::HistoryStepTerminalMetadata,
    state::ChainState,
    AcceptedBlockBundle,
};
use noid_mempool::{AsyncMempool, ChainView, DecodedMempoolIntent, MempoolConfig, SubmitError};
use noid_p2p::object_protocol::{TerminalClaimId, TerminalObjectId};
use noid_poseidon2b::primitives::Address;
use noid_tx::{
    output_bitmap_bit, PagedSpendIntent, Transaction, TxBody, TxInput, TxOutput, TxPage,
    WireError, PAGED_SPEND_END_BIT, PAGED_SPEND_START_BIT, TX_INPUTS, TX_OUTPUTS,
};

#[path = "../../../parano1d/noid_p2p/src/mempool_recovery.rs"]
mod mempool_recovery;
use mempool_recovery::MempoolRecovery;

fn spend_pages(anchor: [u8; 32]) -> Vec<TxPage> {
    let mut inputs = [TxInput::dummy(); TX_INPUTS];
    inputs[0] = TxInput {
        slot_index: 3,
        amount: 40_100,
        creation_id: 2,
    };
    let mut outputs = [TxOutput::dummy(); TX_OUTPUTS];
    outputs[0] = TxOutput {
        slot_index: 4,
        amount: 100,
        owner: Address([0x44; 32]),
    };
    vec![TxPage::new(TxBody {
        epoch_anchor: anchor,
        fee: 40_000,
        input_owner: Address([0xA5; 32]),
        inputs,
        outputs,
        validity_bitmap: 1 | output_bitmap_bit(0) | PAGED_SPEND_START_BIT | PAGED_SPEND_END_BIT,
        is_coinbase: false,
    })
    .unwrap()]
}

fn funded_view(pages: &[TxPage]) -> ChainView {
    let anchor_height = tx_epoch_anchor_height_for_child(1);
    let mut epoch_header = genesis_header();
    epoch_header.height = anchor_height;
    let mut state = ChainState::with_log_slots(6);
    let body = &pages[0].body;
    state
        .state
        .set_slot(
            body.inputs[0].slot_index,
            SlotValue::with_owner_fields(
                body.inputs[0].amount,
                body.inputs[0].creation_id,
                body.input_owner.as_fields(),
            ),
        )
        .unwrap();
    ChainView::new(
        0,
        HashMap::from([(anchor_height, epoch_header)]),
        1,
        state.state,
    )
}

fn pool_with_funded_spend() -> (AsyncMempool, Vec<TxPage>) {
    let anchor_height = tx_epoch_anchor_height_for_child(1);
    let anchor = block_id(&{
        let mut header = genesis_header();
        header.height = anchor_height;
        header
    });
    let pages = spend_pages(anchor);
    let view = funded_view(&pages);
    assert_eq!(view.user_epoch_anchor_id, anchor);
    (
        AsyncMempool::new(view, MempoolConfig::default().with_capacity(8)),
        pages,
    )
}

async fn recovery_excludes_rejected_txid_for_current_scan() {
    let (pool, pages) = pool_with_funded_spend();
    let bytes = PagedSpendIntent::new(pages, b"not-a-wallet-proof".to_vec())
        .unwrap()
        .to_bytes()
        .unwrap();
    let decoded = DecodedMempoolIntent::from_bytes(&bytes).unwrap();
    let txid = decoded.logical_txid();
    let error = pool.submit_decoded(decoded, bytes).await.unwrap_err();
    assert!(matches!(error, SubmitError::InvalidProof(_)));
    assert_eq!(pool.try_contains(&txid), Some(false));

    let now = Instant::now();
    let local = PeerId::random();
    let peer = PeerId::random();
    let mut recovery = MempoolRecovery::new(local);
    recovery.register(peer, now, true);
    recovery.issued(peer, 1, now);
    assert!(recovery.response(peer, 1, true, true, now));

    // This is the exact ordering in noid_node's MempoolSyncResponse worker:
    // decode and record logical_txid, then submit and discard the error.
    recovery.consumed(peer, 1, &[txid.0], now);
    let excluded = recovery.known(peer, []).unwrap();
    assert_eq!(excluded, vec![txid.0]);
    assert_eq!(pool.try_contains(&txid), Some(false));

    let mut fresh_scan: MempoolRecovery<u64> = MempoolRecovery::new(local);
    fresh_scan.register(peer, now, true);
    assert!(fresh_scan.known(peer, []).unwrap().is_empty());
    println!(
        "RECOVERY_BUG txid={} admission=InvalidProof pool_contains=false current_scan_known={} fresh_scan_known=0",
        hex(txid.0),
        excluded.len()
    );
}

async fn proof_bytes_do_not_change_txid_or_replace_cache() {
    let (pool, pages) = pool_with_funded_spend();
    let first = PagedSpendIntent::new(pages.clone(), b"proof-A".to_vec()).unwrap();
    let second = PagedSpendIntent::new(pages, b"proof-B-longer".to_vec()).unwrap();
    assert_eq!(first.logical_txid(), second.logical_txid());
    let first_bytes = first.to_bytes().unwrap();
    let second_bytes = second.to_bytes().unwrap();
    assert_ne!(first_bytes, second_bytes);

    // Isolate identity/cache behavior from proof cryptography. The hook is
    // deliberately synthetic and is not evidence that either blob is a valid proof.
    let calls = Arc::new(AtomicUsize::new(0));
    let observed_calls = Arc::clone(&calls);
    let pool = pool.with_authorization_verification_executor(Arc::new(move |_task| {
        observed_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }));
    let first_id = pool.submit(first, first_bytes.clone()).await.unwrap();
    let second_error = pool.submit(second, second_bytes).await.unwrap_err();
    assert!(matches!(second_error, SubmitError::AlreadyAdmitted(_)));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(pool.len().await, 1);
    let retained = pool.intent_bytes_missing(&[], 1, 4096, 4096).await;
    assert_eq!(retained, vec![first_bytes]);
    println!(
        "PROOF_IDENTITY txid={} distinct_intents=true second=AlreadyAdmitted verifier_calls=1 retained=proof-A pool_len=1",
        hex(first_id.0)
    );
}

fn terminal_artifacts_are_byte_distinct_but_claim_equivalent() {
    let mut header = genesis_header();
    header.height = 1;
    let mut outputs = [TxOutput::dummy(); TX_OUTPUTS];
    outputs[0] = TxOutput {
        slot_index: 60,
        amount: 100,
        owner: header.miner_address,
    };
    let coinbase = Transaction::new(TxBody {
        epoch_anchor: [0; 32],
        fee: 0,
        input_owner: Address([0; 32]),
        inputs: [TxInput::dummy(); TX_INPUTS],
        outputs,
        validity_bitmap: output_bitmap_bit(0),
        is_coinbase: true,
    });
    let mut block = Block {
        header,
        transactions: vec![coinbase],
    };
    block.header.tx_root = noid_chain::compute_tx_root(&block.transactions);
    let block_bytes = block.to_bytes();
    let semantic = semantic_header_id(&block.header);
    let prefix = HistoryStepTerminalMetadata::new(1, semantic, 0)
        .unwrap()
        .encode_prefix();
    let mut terminal_a = prefix.to_vec();
    terminal_a.extend_from_slice(&[0xAA; 16]);
    let mut terminal_b = terminal_a.clone();
    *terminal_b.last_mut().unwrap() ^= 1;

    let bundle_a = AcceptedBlockBundle::try_from_parts(block_bytes.clone(), terminal_a.clone())
        .expect("public framing A");
    let bundle_b = AcceptedBlockBundle::try_from_parts(block_bytes, terminal_b.clone())
        .expect("public framing B");
    assert_eq!(bundle_a.block_hash(), bundle_b.block_hash());
    let claim = TerminalClaimId {
        height: 1,
        semantic_header_id: semantic,
        proof_class: 0,
    };
    let object_a = TerminalObjectId::from_bytes(claim, &terminal_a).unwrap();
    let object_b = TerminalObjectId::from_bytes(claim, &terminal_b).unwrap();
    assert_eq!(object_a.claim, object_b.claim);
    assert_ne!(object_a.byte_digest, object_b.byte_digest);
    assert!(object_a.matches_bytes(&terminal_a));
    assert!(!object_a.matches_bytes(&terminal_b));
    println!(
        "TERMINAL_IDENTITY block_hash={} same_claim=true distinct_object_ids=true original_matches_mutant=false framing_only=true",
        hex(bundle_a.block_hash())
    );
}

fn canonical_and_duplicate_controls() {
    let anchor = block_id(&genesis_header());
    let pages = spend_pages(anchor);
    let page_bytes = pages[0].to_bytes().unwrap();
    assert!(matches!(
        TxBody::from_bytes(&page_bytes),
        Err(WireError::NonCanonicalBody)
    ));
    assert!(TxPage::from_bytes(&page_bytes).is_ok());
    let mut trailing = page_bytes.to_vec();
    trailing.push(0);
    assert!(matches!(
        TxPage::from_bytes(&trailing),
        Err(noid_tx::PagedSpendError::Wire(WireError::TrailingBytes))
    ));
    let mut bad_bool = page_bytes.to_vec();
    bad_bool[322] = 2;
    assert!(matches!(
        TxPage::from_bytes(&bad_bool),
        Err(noid_tx::PagedSpendError::Wire(WireError::InvalidBool))
    ));

    let mut header = genesis_header();
    header.height = 1;
    let mut outputs = [TxOutput::dummy(); TX_OUTPUTS];
    outputs[0] = TxOutput {
        slot_index: 60,
        amount: 100,
        owner: header.miner_address,
    };
    let coinbase = Transaction::new(TxBody {
        epoch_anchor: [0; 32],
        fee: 0,
        input_owner: Address([0; 32]),
        inputs: [TxInput::dummy(); TX_INPUTS],
        outputs,
        validity_bitmap: output_bitmap_bit(0),
        is_coinbase: true,
    });
    let page_tx = Transaction::new(pages[0].body.clone());
    let transactions = vec![coinbase, page_tx.clone(), page_tx];
    assert!(noid_chain::try_compute_logical_txids(&transactions).is_err());
    assert!(validate_scheduled_block_page_stream(&transactions, 1).is_err());
    assert!(validate_block_slot_conflicts(&transactions).is_err());
    println!(
        "NEGATIVE_CONTROLS trailing=reject invalid_bool=reject duplicate_group=DuplicateInputSlot duplicate_block=SlotConflict"
    );
}

fn address_text_aliases_are_accepted() {
    let address = Address([0x42; 32]);
    let lowercase = address.to_bech32();
    let uppercase = lowercase.to_uppercase();
    let padded = format!(" \t{lowercase}\r\n");
    assert_eq!(Address::parse(&uppercase).unwrap(), address);
    assert_eq!(Address::parse(&padded).unwrap(), address);
    assert_eq!(Address::parse(&uppercase).unwrap().to_bech32(), lowercase);
    println!(
        "ADDRESS_TEXT lowercase=true uppercase=true surrounding_whitespace=true same_32_bytes=true binary_identity_changed=false"
    );
}

fn hex(bytes: [u8; 32]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[tokio::main]
async fn main() {
    println!("PIN 52938433a30b185ab007afaa15166a632cf2e82e");
    recovery_excludes_rejected_txid_for_current_scan().await;
    proof_bytes_do_not_change_txid_or_replace_cache().await;
    terminal_artifacts_are_byte_distinct_but_claim_equivalent();
    address_text_aliases_are_accepted();
    canonical_and_duplicate_controls();
    println!("ALL_ASSERTIONS_PASSED");
}
