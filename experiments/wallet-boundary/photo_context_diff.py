#!/usr/bin/env python3
"""Reproduce the documented versus shipped photo-key derivation on synthetic pixels."""
from pathlib import Path
import re
import subprocess

import blake3


root = Path(__file__).resolve().parents[2] / "parano1d"
pin = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip()
assert pin == "52938433a30b185ab007afaa15166a632cf2e82e", pin

source = (root / "noid_gui/src/secret.rs").read_text()
production = re.search(r'const IMAGE_SECRET_CONTEXT: &str = "([^"]+)";', source)[1]
guides = [root / f"docs/{lang}wallet/photo-key.md" for lang in ("", "i18n/ru/", "i18n/zh/")]
documented = []
for guide in guides:
    text = guide.read_text()
    documented.append(re.search(r'^(Parano1d master secret from canonical image pixels v1)$', text, re.M)[1])
assert len(set(documented)) == 1
assert production != documented[0]

width = height = 32
rgba = bytes(value for index in range(width * height) for value in (index % 64, (index * 3) % 256, (index * 7) % 256, 255))
assert len({tuple(rgba[index:index + 4]) for index in range(0, len(rgba), 4)}) >= 64
material = b"RGBA8" + width.to_bytes(4, "little") + height.to_bytes(4, "little") + len(rgba).to_bytes(8, "little") + rgba

def derive(context: str) -> str:
    digest = blake3.blake3(derive_key_context=context)
    digest.update(material)
    return digest.hexdigest()

shipped = derive(production)
from_docs = derive(documented[0])
assert shipped != from_docs
print(f"pin={pin}")
print(f"production_context={production!r}")
print(f"documented_context={documented[0]!r} (all three guides)")
print(f"fixture={width}x{height} RGBA, {len(set(rgba[i:i+4] for i in range(0, len(rgba), 4)))} distinct pixels")
print(f"shipped_master_secret={shipped}")
print(f"documented_master_secret={from_docs}")
address_bin = Path(__file__).resolve().parent / "target/photo-address"
if address_bin.is_file():
    address_lines = subprocess.check_output([str(address_bin), shipped, from_docs], text=True).splitlines()
    assert len(address_lines) == 2 and address_lines[0] != address_lines[1]
    for line in address_lines:
        print(line)
print("RESULT: identical image pixels produce different spending keys and addresses when implemented from the published guide.")
