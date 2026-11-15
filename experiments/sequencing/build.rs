// SPDX-License-Identifier: Apache-2.0
// Verify the audited checkout is at the pin and that every ordering fact the
// PoC relies on exists verbatim in the pinned sources (read via `git show`,
// never from a possibly-modified working tree).

use std::path::{Path, PathBuf};
use std::process::Command;

const PIN: &str = "52938433a30b185ab007afaa15166a632cf2e82e";

fn git(repo: &Path, args: &[&str]) -> Vec<u8> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn pinned<'a>(repo: &Path, file: &str) -> String {
    let bytes = git(repo, &["show", &format!("{PIN}:{file}")]);
    String::from_utf8(bytes).unwrap_or_else(|_| panic!("read {file} as utf-8"))
}

fn expect_contains(source: &str, needle: &str, what: &str) {
    assert!(
        source.contains(needle),
        "pinned source fact missing: {what}"
    );
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:warning=verified pinned fact: {what}");
}

fn main() {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let repo = manifest.join("../../parano1d");
    let head = String::from_utf8(git(&repo, &["rev-parse", "HEAD"])).unwrap();
    assert_eq!(
        head.trim(),
        PIN,
        "the audited repository must be at the pin"
    );

    // 1. Mempool fee index: descending fee_rate, ascending logical txid.
    let mempool = pinned(&repo, "noid_chain/src/mempool.rs");
    expect_contains(
        &mempool,
        "/// BTreeMap iterates in ascending key order, so we use descending fee_rate",
        "mempool FeeKey: descending fee_rate",
    );
    expect_contains(
        &mempool,
        "/// Ascending logical txid: deterministic tie-break.",
        "mempool FeeKey: ascending txid tie-break",
    );
    // No replacement policy anywhere in the pool.
    assert!(
        !mempool.contains("fn replace"),
        "pinned mempool must not grow a replacement path for this audit"
    );

    // 2. Template builder sort: contract-first, descending absolute fee,
    //    ascending logical txid.
    let template = pinned(&repo, "noid_chain/src/consensus/template.rs");
    let sort_start = template
        .find("candidates.sort_by(|left, right| {")
        .expect("template sort comparator");
    let comparator = &template[sort_start..];
    let comparator = &comparator[..comparator.find("\n    });").unwrap()];
    expect_contains(
        comparator,
        "contract(right).cmp(&contract(left))",
        "template sort: contract groups first",
    );
    expect_contains(
        comparator,
        ".spend\n                .fee\n                .cmp(&left.spend.fee)",
        "template sort: descending absolute fee",
    );
    expect_contains(
        comparator,
        ".spend.logical_txid.0.cmp(&right.spend.logical_txid.0)",
        "template sort: ascending txid tie-break",
    );

    // 3. Consensus validation imposes no tx ordering rule.
    let validation = pinned(&repo, "noid_chain/src/consensus/validation.rs");
    let v_start = validation
        .find("pub fn validate_block_consensus(")
        .expect("validate_block_consensus");
    let v_body = &validation[v_start..validation.len()];
    let v_end = v_body.find("\n#[cfg(test)]").map(|o| o + v_start).unwrap();
    let body = &validation[v_start..v_end];
    assert!(
        !body.contains("sort") && !body.contains("fee.cmp") && !body.contains("order"),
        "validate_block_consensus must not enforce an ordering rule for this audit's claim"
    );

    // 4. Miner refresh freeze: mid-interval refresh only while user_pages == 0.
    let miner = pinned(&repo, "noid_miner/src/miner.rs");
    expect_contains(
        &miner,
        "_ = &mut heartbeat, if user_pages == 0 => {",
        "miner: heartbeat refresh gated on empty template",
    );
    expect_contains(
        &miner,
        "refresh = next_mining_mempool_refresh(&mut mempool_events), if user_pages == 0 => {",
        "miner: TxAdmitted refresh gated on empty template",
    );

    // 5. Financial natives pay exactly the parent-derived fee (no fee lever).
    let native = pinned(&repo, "noid_mempool/src/native.rs");
    expect_contains(
        &native,
        "if matches!(authorization, NativeAuthorizationContext::Financial(_)) && facts.fee != minimum {",
        "mempool: financial native fee must equal the exact minimum",
    );
    let financial = pinned(&repo, "noid_chain/src/block/native_assets/financial.rs");
    expect_contains(
        &financial,
        "if context.fee != fee {",
        "consensus: financial fee equality with parent-derived requirement",
    );

    // 6. Trade priority key: TRORD_21 over (parent_id, asset, creation_id,
    //    slot_index) of the principal funding anchor.
    let trade = pinned(&repo, "noid_tx/src/native_assets/trade.rs");
    let fp_start = trade
        .find("pub fn funding_priority")
        .expect("funding_priority");
    let fp = &trade[fp_start
        ..trade
            .find("pub fn payouts")
            .expect("payouts after priority")];
    expect_contains(
        fp,
        "DomainTag::new(b\"TRORD_21\")",
        "trade priority domain TRORD_21",
    );
    expect_contains(
        fp,
        "Address(parent_id).as_fields()",
        "trade priority binds parent_id",
    );
    expect_contains(
        fp,
        "Block128(u128::from(anchor.input.creation_id))",
        "trade priority binds anchor creation_id",
    );
    expect_contains(
        fp,
        "Block128(u128::from(anchor.input.slot_index))",
        "trade priority binds anchor slot_index",
    );

    // 7. Trade batch canonical order and execution order keys.
    let batch = pinned(&repo, "noid_tx/src/native_assets/trade/batch.rs");
    expect_contains(
        &batch,
        "keyed.sort_unstable_by_key(|(key, _)| *key);",
        "trade batch canonical encoding sorted by authorization id",
    );
    let sequencing = pinned(&repo, "noid_tx/src/native_assets/trade/sequencing.rs");
    expect_contains(
        &sequencing,
        "remaining.sort_unstable_by_key(|&index| priorities[index]);",
        "execution order sorted by funding priority",
    );
    let trades_template = pinned(&repo, "noid_miner/src/template/trades.rs");
    expect_contains(
        &trades_template,
        "ranked.sort_unstable_by_key(|(priority, _)| *priority);",
        "builder ranks trade intents by funding priority",
    );

    // 8. NFT payer priority: NMPRIOR1 over (round, payer address).
    let nft = pinned(&repo, "noid_tx/src/native_assets/nft_market.rs");
    let pp_start = nft.find("pub fn payer_priority").expect("payer_priority");
    let pp_tail = &nft[pp_start..];
    let pp = &pp_tail[..pp_tail.find("\n    }\n").map(|o| o + 7).unwrap()];
    expect_contains(
        pp,
        "DomainTag::new(b\"NMPRIOR1\")",
        "NFT payer priority domain NMPRIOR1",
    );
    expect_contains(
        pp,
        "self.payer.as_fields()",
        "NFT payer priority binds payer address",
    );

    // 9. Fee model: state-growth component priced separately and burned.
    let fees = pinned(&repo, "noid_chain/src/consensus/fees.rs");
    expect_contains(
        &fees,
        "pub fn fee_breakdown(",
        "deterministic fee breakdown entry point",
    );
}
