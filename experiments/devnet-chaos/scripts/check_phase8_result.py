#!/usr/bin/env python3
"""Extract and check the saved phase-8 release-node verdict."""

from pathlib import Path
import re


root = Path(__file__).resolve().parents[1]
ansi = re.compile(r"\x1b\[[0-9;]*m")
results = (root / "results.txt").read_text().split("===== PHASE 8")[-1]
victim = [ansi.sub("", line) for line in (root / "logs/p8-victim.log").read_text().splitlines()]
next_joiner = [ansi.sub("", line) for line in (root / "logs/p8-victim2.log").read_text().splitlines()]


def first(lines: list[str], *fragments: str) -> str:
    return next(line for line in lines if all(fragment in line for fragment in fragments))


checks = [
    ("poison offer selected", first(victim, "candidate selection settled", "tip=42")),
    ("poison inventory rejected", first(victim, "header inventory failed native validation", "BadTimestamp")),
    ("poison terminal rejected", first(victim, "snapshot candidate rejected", "HistoryStep target tip exceeds local future drift")),
    ("honest state installed", first(victim, "snapshot boundary State installed", "snapshot_height=12")),
    ("honest suffix selected", first(victim, "post-snapshot exact tail pinned", "target_height=31")),
    ("second joiner installed state", first(next_joiner, "snapshot boundary State installed", "snapshot_height=12")),
    ("second joiner verified suffix", first(next_joiner, "exact suffix terminal verified", "height=31")),
]

assert "[phase8] RESULT: victim_on_poison=False reserved_to_next=False" in results
assert "[phase8] victim final tip (31, 'd6fc8017364927c8')" in results
assert "[phase8] second joiner final tip (31, 'd6fc8017364927c8')" in results

output = ["Phase-8 release-node verdict, extracted from the saved local run", ""]
for label, line in checks:
    output.append(f"{label}: {line}")
output.extend([
    "",
    first(results.splitlines(), "[phase8] victim final tip"),
    first(results.splitlines(), "[phase8] second joiner final tip"),
    first(results.splitlines(), "[phase8] RESULT:"),
    "",
    "PASS: both fresh joiners adopted honest height 31; poison acceptance and re-serve were false.",
])
print("\n".join(output))
