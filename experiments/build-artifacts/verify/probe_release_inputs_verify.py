#!/usr/bin/env python3
"""Independent re-verification of the agent-25 HIGH finding.

Substitutes legacy matrix bytes (same decompressed length) in a legacy pack,
leaves pins.env + SHA256SUMS stale, and checks whether the standalone release
builder (scripts/build_release.sh, byte-identical fixture) still reaches Cargo
with the unchanged metadata pin. Adds two controls the original probe lacked:

  1. an unsubstituted run (does the pristine pack also reach cargo? it must,
     else the substituted run proves nothing);
  2. a scan of the build log for any SHA-256/manifest verification activity
     over the legacy pack during the substituted run.
"""
import hashlib
import json
import os
import re
from pathlib import Path
import subprocess
import tempfile

LANE = Path(__file__).resolve().parent.parent          # experiments/build-artifacts
ROOT = LANE / "fixture"                                 # byte-identical to pinned checkout
VERIFY = LANE / "verify"
case = Path(tempfile.mkdtemp(prefix="verify-legacy-sub-", dir=LANE / "tmp"))

legacy = case / "legacy"
v2 = case / "v2"
v21 = case / "v21"
keys = case / "keys"
bin_dir = case / "bin"
for directory in (legacy / "v1", v2, v21, keys, bin_dir):
    directory.mkdir(parents=True)

def zstd(data):
    return subprocess.run(["zstd", "-q", "-c"], input=data, capture_output=True, check=True).stdout

metadata = b"layout fixture metadata"
original = zstd(b"A" * 1024)
substituted = zstd(b"B" * 1024)
other = zstd(b"C" * 1024)
(legacy / "v1/history-step.runtime").write_bytes(metadata)
(legacy / "v1/history-step-c00.field-r1cs.zst").write_bytes(original)
(legacy / "v1/history-step-c01.field-r1cs.zst").write_bytes(other)
metadata_pin = "a1" * 32
leaf_pins = "a2" * 64
(legacy / "pins.env").write_text(
    f"NOID_HISTORY_STEP_RUNTIME_METADATA_RELEASE_DIGEST={metadata_pin}\n"
    f"NOID_HISTORY_STEP_PACK_LEAF_DIGESTS={leaf_pins}\n"
)
paths = [
    "v1/history-step.runtime",
    "v1/history-step-c00.field-r1cs.zst",
    "v1/history-step-c01.field-r1cs.zst",
    "pins.env",
]
(legacy / "SHA256SUMS").write_text("".join(
    f"{hashlib.sha256((legacy / path).read_bytes()).hexdigest()}  {path}\n" for path in paths
))
for name in ("v2-runtime-metadata.bin", "v2-small.field-r1cs.zst", "v2-large.field-r1cs.zst"):
    (v2 / name).write_bytes(b"layout fixture")
for name in ("v21-runtime-metadata.bin", "v21-small.field-r1cs.zst", "v21-large.field-r1cs.zst", "v21-native.field-r1cs.zst"):
    (v21 / name).write_bytes(b"layout fixture")
for name in ("class-0.key", "class-1.key"):
    (keys / name).write_bytes(b"layout fixture")
v2_pins = case / "v2-pins.env"
v2_pins.write_text(
    "NOID_V2_RELEASE_BANK=" + "b1" * 32 + "\n"
    "NOID_RETIREMENT_KEY_0_PIN=" + "c0" * 32 + "\n"
    "NOID_RETIREMENT_KEY_1_PIN=" + "c1" * 32 + "\n"
)
v21_pins = case / "v21-pins.env"
v21_pins.write_text("NOID_V21_RELEASE_BANK=" + "d1" * 32 + "\n")

capture = case / "captured.json"
cargo = bin_dir / "cargo"
cargo.write_text("""#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
if sys.argv[1] == 'pkgid':
    print('path+file:///fixture#' + sys.argv[-1] + '@2.1.0')
elif sys.argv[1] == '--version':
    print('cargo fixture')
elif sys.argv[1] == 'build':
    Path(os.environ['TEST_ENV_CAPTURE']).write_text(json.dumps({
        'args': sys.argv[1:],
        'env': {k: v for k, v in os.environ.items() if k.startswith('NOID_')}}))
    sys.exit(91)
else:
    sys.exit(92)
""")
cargo.chmod(0o755)
rustc = bin_dir / "rustc"
rustc.write_text("#!/bin/sh\ncase \"$1\" in -vV) echo 'host: x86_64-unknown-linux-gnu';; *) echo 'rustc fixture';; esac\n")
rustc.chmod(0o755)

def run_build(tag):
    env = os.environ.copy()
    env.update(
        PATH=str(bin_dir) + os.pathsep + env["PATH"],
        TEST_ENV_CAPTURE=str(case / f"captured-{tag}.json"),
    )
    command = [
        "bash", str(ROOT / "scripts/build_release.sh"),
        "--pack", str(legacy), "--v2-pack", str(v2), "--v2-pins", str(v2_pins),
        "--v21-pack", str(v21), "--v21-pins", str(v21_pins),
        "--retirement-keys", str(keys), "--output", str(case / f"output-{tag}"),
    ]
    result = subprocess.run(command, cwd=ROOT, env=env, capture_output=True, text=True, timeout=60)
    log = result.stdout + result.stderr
    captured_path = case / f"captured-{tag}.json"
    captured = json.loads(captured_path.read_text()) if captured_path.exists() else None
    return result, log, captured

# Control run: pristine pack must also reach cargo (exit 91 = stub's code).
control_result, control_log, control_captured = run_build("control")

# Substituted run: swap c00 matrix bytes for different valid zstd with the same
# decompressed length; pins.env and SHA256SUMS deliberately left stale.
(legacy / "v1/history-step-c00.field-r1cs.zst").write_bytes(substituted)
sub_result, sub_log, sub_captured = run_build("substituted")

verification_activity = sorted(set(
    line.strip() for line in sub_log.splitlines()
    if re.search(r"sha256|SHA-256|manifest|pins\.env mismatch|digest", line, re.I)
    and "matrix pack:" not in line
))

verify = subprocess.run(
    ["bash", "-c", f"source '{ROOT / 'scripts/release_common.sh'}'; release_verify_sha256_manifest '{legacy}'"],
    cwd=ROOT, capture_output=True, text=True,
)

report = {
    "case": str(case),
    "fixture_matches_checkout": subprocess.run(
        ["diff", str(ROOT / "scripts/build_release.sh"),
         str(LANE.parent.parent / "parano1d/scripts/build_release.sh")],
        capture_output=True).returncode == 0,
    "original_c00_sha256": hashlib.sha256(original).hexdigest(),
    "substituted_c00_sha256": hashlib.sha256(substituted).hexdigest(),
    "decompressed_lengths": [1024, 1024],
    "control": {
        "build_release_exit": control_result.returncode,
        "cargo_reached": control_captured is not None,
        "metadata_pin_seen_by_cargo": control_captured is not None and control_captured["env"].get("NOID_HISTORY_STEP_RUNTIME_METADATA_RELEASE_DIGEST"),
    },
    "substituted": {
        "build_release_exit": sub_result.returncode,
        "cargo_reached": sub_captured is not None,
        "metadata_pin_seen_by_cargo": sub_captured is not None and sub_captured["env"].get("NOID_HISTORY_STEP_RUNTIME_METADATA_RELEASE_DIGEST"),
        "leaf_pin_exported_to_cargo": sub_captured is not None and "NOID_HISTORY_STEP_PACK_LEAF_DIGESTS" in sub_captured["env"],
        "pack_dir_seen_by_cargo": sub_captured is not None and sub_captured["env"].get("NOID_HISTORY_STEP_PACK_DIR"),
        "manifest_stale_for_c00": next(
            line.split()[0] for line in (legacy / "SHA256SUMS").read_text().splitlines()
            if line.endswith("v1/history-step-c00.field-r1cs.zst")
        ) != hashlib.sha256(substituted).hexdigest(),
        "log_lines_mentioning_hash_or_manifest": verification_activity,
    },
    "explicit_manifest_verifier_exit": verify.returncode,
    "explicit_manifest_verifier_stderr": verify.stderr.strip(),
    "substituted_build_log_tail": sub_log.splitlines()[-8:],
}

assert control_result.returncode == 91, control_log
assert control_captured is not None
assert sub_result.returncode == 91, sub_log
assert sub_captured is not None
assert sub_captured["env"]["NOID_HISTORY_STEP_RUNTIME_METADATA_RELEASE_DIGEST"] == metadata_pin
assert "NOID_HISTORY_STEP_PACK_LEAF_DIGESTS" not in sub_captured["env"]
assert sub_captured["env"]["NOID_HISTORY_STEP_PACK_DIR"] == str(legacy)
assert verify.returncode != 0
assert "history-step-c00" in verify.stderr

output = VERIFY / "release-inputs-verify.json"
output.write_text(json.dumps(report, indent=2) + "\n")
print(json.dumps(report, indent=2))
