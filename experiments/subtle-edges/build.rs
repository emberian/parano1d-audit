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

fn main() {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let repo = manifest.join("../../parano1d");
    let head = String::from_utf8(git(&repo, &["rev-parse", "HEAD"])).unwrap();
    assert_eq!(
        head.trim(),
        PIN,
        "the audited repository must be at the pin"
    );

    // Pin the exact source facts this lane asserts against, so a silent
    // upstream edit cannot make the compiled constants drift from the cited
    // file:line evidence.
    let params = String::from_utf8(git(
        &repo,
        &["show", &format!("{PIN}:noid_chain/src/consensus/params.rs")],
    ))
    .unwrap();
    for (needle, what) in [
        (
            "pub const MAINNET_V2_1_ACTIVATION_HEIGHT: Option<u64> = Some(279_370);",
            "v2.1 mainnet activation height",
        ),
        (
            "pub const MAINNET_V2_ACTIVATION_HEIGHT: u64 = 210_537;",
            "v2 mainnet activation height",
        ),
        (
            "pub const CONSENSUS_FINALITY_DEPTH: u64 = 18;",
            "finality depth",
        ),
        ("pub const TX_EPOCH_BLOCKS: u64 = 144;", "tx epoch length"),
        ("pub const EPOCH_LENGTH: u64 = 6;", "ASERT epoch length"),
        ("pub const MAX_FUTURE_DRIFT: u64 = 120;", "future drift"),
    ] {
        assert!(params.contains(needle), "pinned source changed: {what}");
    }

    let difficulty = String::from_utf8(git(
        &repo,
        &[
            "show",
            &format!("{PIN}:noid_chain/src/consensus/difficulty.rs"),
        ],
    ))
    .unwrap();
    assert!(
        difficulty.contains("if net >= 46 {"),
        "pinned difficulty short-circuit changed"
    );

    println!("cargo:rerun-if-changed=build.rs");
}
