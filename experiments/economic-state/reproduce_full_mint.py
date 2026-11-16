#!/usr/bin/env python3
"""Derive a locked-NFT control from the existing production-code mint PoC.

The temporary crate is isolated; this script never edits the source lane or
the pinned target. The original PoC already tests block admission with normal
unlock heights; this variant tests the consensus transition with MAX height.
"""
from pathlib import Path
import os
import shutil
import subprocess

lane = Path(__file__).resolve().parent
root = lane.parents[1]
source = root / "experiments/creation-spam/poc"
temporary = lane / "tmp/full-mint"
temporary.mkdir(parents=True, exist_ok=True)
shutil.copy2(source / "Cargo.toml", temporary / "Cargo.toml")
shutil.copy2(source / "Cargo.lock", temporary / "Cargo.lock")
(temporary / "src").mkdir(exist_ok=True)
text = (source / "src/main.rs").read_text()
old = """                unlock_height: 0,
            }),
        ));
    }
    outputs.push(ordinary_output("""
assert text.count(old) == 1
text = text.replace(old, old.replace("unlock_height: 0", "unlock_height: u64::MAX"))
old = """    let result = mint_batch(&mut state, &collection, cslot, fund, 3, address(VICTIM));
    println!("""
assert text.count(old) == 1
new = """    let mint_slots = fresh_slots(&state, 5);
    let result = mint_batch(&mut state, &collection, cslot, fund, 3, address(VICTIM));
    let first = NativeOpening::NftItem(NftItem {
        collection: collection.collection,
        owner: address(VICTIM),
        metadata: [0xB0; 32],
        serial: collection.issued_items + 1,
        unlock_height: u64::MAX,
    });
    let claim = native_input(&state, mint_slots[1], first);
    assert!(matches!(first.check_input(&claim.input, claim.root, HEIGHT),
        Err(noid_tx::native_assets::NativeAssetError::Timelock)));
    println!("MAX-locked victim NFT minted into real ChainState; victim cannot spend at activation height");
    println!("""
text = text.replace(old, new)
old = """fn main() {
    phase1_inventory();
    phase2_ratio();
    match phase3_mdbx() {"""
assert text.count(old) == 1
text = text.replace(old, """fn main() {
    phase1_inventory();
    return;
    phase2_ratio();
    match phase3_mdbx() {""")
(temporary / "src/main.rs").write_text(text)
env = os.environ.copy()
env["CARGO_TARGET_DIR"] = str(root / "target")
subprocess.run(
    ["cargo", "run", "--release", "-j", "1", "--manifest-path", str(temporary / "Cargo.toml")],
    check=True,
    env=env,
)
