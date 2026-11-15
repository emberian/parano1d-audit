#!/usr/bin/env python3
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile

LANE = Path(__file__).resolve().parent
ROOT = LANE / "fixture"
case = Path(tempfile.mkdtemp(prefix="legacy-substitution-", dir=LANE / "tmp"))
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
(legacy / "v1/history-step.runtime").write_bytes(metadata)
(legacy / "v1/history-step-c00.field-r1cs.zst").write_bytes(original)
(legacy / "v1/history-step-c01.field-r1cs.zst").write_bytes(zstd(b"C" * 1024))
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
        'env': {k: v for k, v in os.environ.items() if k.startswith('NOID_') or k == 'CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS'}}))
    sys.exit(91)
else:
    sys.exit(92)
""")
cargo.chmod(0o755)
rustc = bin_dir / "rustc"
rustc.write_text("#!/bin/sh\ncase \"$1\" in -vV) echo 'host: x86_64-unknown-linux-gnu';; *) echo 'rustc fixture';; esac\n")
rustc.chmod(0o755)

(legacy / "v1/history-step-c00.field-r1cs.zst").write_bytes(substituted)
env = os.environ.copy()
env.update(
    PATH=str(bin_dir) + os.pathsep + env["PATH"],
    TEST_ENV_CAPTURE=str(capture),
    CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS="true",
)
command = [
    "bash", str(ROOT / "scripts/build_release.sh"),
    "--pack", str(legacy), "--v2-pack", str(v2), "--v2-pins", str(v2_pins),
    "--v21-pack", str(v21), "--v21-pins", str(v21_pins),
    "--retirement-keys", str(keys), "--output", str(case / "output"),
]
result = subprocess.run(command, cwd=ROOT, env=env, capture_output=True, text=True, timeout=30)
captured = json.loads(capture.read_text()) if capture.exists() else None
verify = subprocess.run(
    ["bash", "-c", f"source '{ROOT / 'scripts/release_common.sh'}'; release_verify_sha256_manifest '{legacy}'"],
    cwd=ROOT, capture_output=True, text=True,
)
report = {
    "case": str(case),
    "original_c00_sha256": hashlib.sha256(original).hexdigest(),
    "substituted_c00_sha256": hashlib.sha256(substituted).hexdigest(),
    "original_and_substituted_canonical_lengths": [1024, 1024],
    "pins_env_unchanged": (legacy / "pins.env").read_text(),
    "build_release_exit": result.returncode,
    "cargo_reached": captured is not None,
    "cargo_profile_override_seen_by_cargo": captured is not None and captured["env"].get("CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS"),
    "legacy_metadata_pin_seen_by_cargo": captured is not None and captured["env"].get("NOID_HISTORY_STEP_RUNTIME_METADATA_RELEASE_DIGEST"),
    "legacy_leaf_pin_exported_to_cargo": captured is not None and "NOID_HISTORY_STEP_PACK_LEAF_DIGESTS" in captured["env"],
    "explicit_manifest_verifier_exit": verify.returncode,
    "explicit_manifest_verifier_stderr": verify.stderr.strip(),
    "build_log_tail": (result.stdout + result.stderr).splitlines()[-8:],
}
assert result.returncode == 91, result.stdout + result.stderr
assert captured is not None
assert captured["env"]["CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS"] == "true"
assert verify.returncode != 0
output = LANE / "probes/release-inputs.json"
output.write_text(json.dumps(report, indent=2) + "\n")
print(json.dumps(report, indent=2))
