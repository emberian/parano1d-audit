//! Narrow check of the production opening's output and input rules.
//! Compile with the existing pinned release rlibs; see REPORT.md.
use noid_poseidon2b::primitives::Address;
use noid_tx::native_assets::{native_creation_id, CollectionId, NativeOpening, NftItem};
use noid_tx::{TxInput, TxOutput};

fn main() {
    let victim = Address([0xDD; 32]);
    let item = NativeOpening::NftItem(NftItem {
        collection: CollectionId([0x44; 32]),
        owner: victim,
        metadata: [0x55; 32],
        serial: 1,
        unlock_height: u64::MAX,
    });
    let carrier = TxOutput {
        slot_index: 73,
        amount: 0,
        owner: item.root(),
    };
    item.check_output(&carrier).unwrap();
    let input = TxInput {
        slot_index: carrier.slot_index,
        amount: carrier.amount,
        creation_id: native_creation_id(42).unwrap(),
    };
    item.check_input_binding(&input, carrier.owner).unwrap();
    assert!(matches!(
        item.check_input(&input, carrier.owner, 279_370),
        Err(noid_tx::native_assets::NativeAssetError::Timelock)
    ));
    assert!(matches!(
        item.check_input(&input, carrier.owner, u64::MAX - 1),
        Err(noid_tx::native_assets::NativeAssetError::Timelock)
    ));
    item.check_input(&input, carrier.owner, u64::MAX).unwrap();
    println!("accepted output: owner=unsolicited victim, unlock_height=u64::MAX");
    println!("input binding valid but spends fail Timelock at h=279370 and h=MAX-1");
    println!("input accepted only at h=MAX");
}
