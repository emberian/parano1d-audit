#!/usr/bin/env python3
"""Check the saved phase-7 evidence, without starting or contacting any node."""
from pathlib import Path
import hashlib
import re

lane = Path(__file__).resolve().parents[1]
log = lane / "logs/n7s-a.log"
raw = log.read_bytes()
lines = re.sub(r"\x1b\[[0-9;]*m", "", raw.decode()).splitlines()
print(f"source: {log.relative_to(lane)}")
print(f"sha256: {hashlib.sha256(raw).hexdigest()}")
hits = []
for number, line in enumerate(lines, 1):
    if "HeaderDAG-selected tip is discovering exact object providers" in line:
        local = re.search(r"our_height=(\d+)", line)
        selected = re.search(r"target_height=(\d+)", line)
        if local and selected and int(local[1]) == 0 and int(selected[1]) > 0:
            hits.append((int(selected[1]), number, line))
assert hits, "Saved log does not establish selected headers ahead of installed state"
for height in sorted({hit[0] for hit in hits}):
    _, number, line = next(hit for hit in hits if hit[0] == height)
    print(f"L{number}: {line}")
assert max(hit[0] for hit in hits) == 150
print("VERDICT: installed height zero coexists with HeaderDAG-selected height 150.")
print("Phase 7.3 does not establish the claimed genesis-only HeaderDAG window.")
print("This is a saved-evidence check, not a new live reproduction.")
