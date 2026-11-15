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

    let source = String::from_utf8(git(
        &repo,
        &["show", &format!("{PIN}:noid_rpc/src/server.rs")],
    ))
    .unwrap();
    let start = source
        .find("struct CachedExternalMiningTemplate<T> {")
        .expect("attempt-slot start marker");
    let end = source
        .find("/// A small node-facing handle which immediately drops a ready attempt")
        .expect("attempt-slot end marker");
    assert!(start < end);
    let mut extracted = source[start..end].to_owned();

    let submit = source
        .split("async fn submit_block(")
        .nth(1)
        .expect("submitBlock handler");
    let begin = submit.find(".begin_proving(").unwrap();
    let finalize = submit
        .find(".finalize_node_owned_template_after_nonce(")
        .unwrap();
    assert!(
        begin < finalize,
        "PoC expects submission to consume the slot before finalization"
    );
    let finalizer = source
        .split("async fn finalize_node_owned_template_after_nonce(")
        .nth(1)
        .expect("nonce finalizer");
    assert!(
        finalizer
            .find("validate_pow(&prepared.pow_header(nonce))")
            .is_some(),
        "PoC expects the finalizer to perform the real PoW check"
    );
    extracted.push_str("\nconst PINNED_CONSUME_BEFORE_POW_VERIFIED: bool = true;\n");

    println!("cargo:rerun-if-changed=build.rs");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    std::fs::write(out.join("pinned_external_mining_slot.rs"), extracted).unwrap();
}
