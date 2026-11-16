#!/usr/bin/env python3
"""Time real verified wallet pages on the existing locked-NFT local fixture.

The generated crate and MDBX database remain inside this lane's ignored tmp/.
Run after `economic-state/reproduce_full_mint.py` has generated its fixture
source. No pinned target source or public network is modified or contacted.
"""

from pathlib import Path
import os
import shutil
import subprocess

lane = Path(__file__).resolve().parent
root = lane.parents[1]
source = root / "experiments/economic-state/tmp/full-mint"
temporary = lane / "tmp/page-timing"
(temporary / "src").mkdir(parents=True, exist_ok=True)
for name in ("Cargo.toml", "Cargo.lock"):
    shutil.copy2(source / name, temporary / name)
text = (source / "src/main.rs").read_text()
old = '''    std::fs::create_dir_all("../target").map_err(|e| e.to_string())?;
    let directory = tempfile::Builder::new()
        .prefix("creation-spam-")
        .tempdir_in("../target")
'''
new = '''    std::fs::create_dir_all("../").map_err(|e| e.to_string())?;
    let directory = tempfile::Builder::new()
        .prefix("wallet-pages-")
        .tempdir_in("../")
'''
assert text.count(old) == 1
text = text.replace(old, new)
anchor = '    println!("CONTROL victim wallet: {wallet_items} MAX-locked NFTs across {wallet_pages} verified 64-item pages; every input rejected Timelock");\n'
assert text.count(anchor) == 1
probe = '''    // Warm-store timings of the production verified wallet query. These
    // exclude JSON-RPC transport, GUI decode, gallery rendering, and blocks.
    let mut page_us = Vec::new();
    let mut scan_us = Vec::new();
    for _ in 0..100 {
        let started_scan = Instant::now();
        let mut cursor = None;
        let mut count = 0;
        loop {
            let started_page = Instant::now();
            let page = store.native_wallet_page(address(VICTIM), cursor, 64)
                .map_err(|e| format!("timed wallet page: {e}"))?;
            page_us.push(started_page.elapsed().as_micros());
            count += page.items.len();
            cursor = page.next;
            if cursor.is_none() { break; }
        }
        assert_eq!(count, 248);
        scan_us.push(started_scan.elapsed().as_micros());
    }
    page_us.sort_unstable();
    scan_us.sort_unstable();
    println!("MEASURE warm verified wallet page (64 items; n={}): p50={}us p95={}us min={}us max={}us", page_us.len(), page_us[page_us.len()/2], page_us[page_us.len()*95/100], page_us[0], page_us[page_us.len()-1]);
    println!("MEASURE warm verified wallet full scan (248 items; n={}): p50={}us p95={}us min={}us max={}us", scan_us.len(), scan_us[scan_us.len()/2], scan_us[scan_us.len()*95/100], scan_us[0], scan_us[scan_us.len()-1]);
    let mut empty_us = Vec::new();
    for _ in 0..100 {
        let started = Instant::now();
        let page = store.native_wallet_page(address(0xAB), None, 64)
            .map_err(|e| format!("empty wallet page: {e}"))?;
        assert!(page.items.is_empty());
        empty_us.push(started.elapsed().as_micros());
    }
    empty_us.sort_unstable();
    println!("CONTROL warm empty wallet page (n={}): p50={}us p95={}us", empty_us.len(), empty_us[empty_us.len()/2], empty_us[empty_us.len()*95/100]);
'''
text = text.replace(anchor, anchor + probe)
anchor = '''    Ok(())
}

fn main() {
    phase1_inventory();'''
assert text.count(anchor) == 1
text = text.replace(anchor, '''    println!("FIXTURE_DB={}", directory.path().canonicalize().unwrap().display());
    println!("FIXTURE_VICTIM={}", address(VICTIM).to_bech32());
    println!("FIXTURE_EMPTY={}", address(0xAB).to_bech32());
    std::mem::forget(directory); // preserve this isolated fixture for later inspection
    Ok(())
}

fn main() {
    phase1_inventory();''')
(temporary / "src/main.rs").write_text(text)
env = os.environ.copy()
env["CARGO_TARGET_DIR"] = str(root / "target")
result = subprocess.run(
    ["cargo", "run", "--release", "-j", "1", "--manifest-path", str(temporary / "Cargo.toml")],
    check=True,
    env=env,
    cwd=temporary,
    text=True,
    stdout=subprocess.PIPE,
    stderr=subprocess.STDOUT,
)
print(result.stdout, end="", flush=True)
(lane / "page-timing-results.txt").write_text(result.stdout)
