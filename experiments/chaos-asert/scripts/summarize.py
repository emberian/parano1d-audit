#!/usr/bin/env python3
"""Summarize run/measurements.jsonl into the REPORT.md tables.

Outputs: raise curve (epoch-anchor targets + cadence by segment), freeze
numbers, fork block table, wait samples, adoption timings, post-collapse
production blocks with cadence and targets.
"""

import json
import statistics
import sys
from pathlib import Path

RUN = Path(__file__).resolve().parents[1] / "run"
FLOOR = 1 << 238


def rows():
    path = RUN / "measurements.jsonl"
    if not path.exists():
        print("no measurements yet")
        sys.exit(0)
    with open(path) as fh:
        for line in fh:
            line = line.strip()
            if line:
                yield json.loads(line)


def fmt_ratio(target_int):
    if target_int is None:
        return "?"
    if target_int == FLOOR:
        return "floor (2^238)"
    return f"2^{FLOOR.bit_length() - 1 - target_int.bit_length() + 1:.1f}x floor-ish  (0x{target_int:064x})"


def main():
    blocks = [r for r in rows() if r["kind"] == "block"]
    fork = [r for r in rows() if r["kind"] == "fork_block"]
    waits = [r for r in rows() if r["kind"] == "wait_sample"]
    print(f"=== raise blocks sampled: {len(blocks)}")
    if blocks:
        segs = {}
        for r in blocks:
            segs.setdefault(r["producer"], []).append(r)
        for producer, rs in segs.items():
            first, last = rs[0], rs[-1]
            dts = []
            prev = None
            for r in rs:
                if prev is not None:
                    dts.append(r["wall"] - prev)
                prev = r["wall"]
            print(f"  {producer}: h{first['height']}..h{last['height']}"
                  f"  ratio_log2 first={first.get('ratio_log2')} last={last.get('ratio_log2')}"
                  f"  cadence mean={statistics.mean(dts):.1f}s median={statistics.median(dts):.1f}s"
                  if dts else f"  {producer}: (no cadence)")
        print("  raise target curve (every 6th block):")
        for r in blocks:
            if r["height"] % 6 == 0:
                print(f"    h{r['height']:>4}  ts={r['ts']}  ratio_log2={r.get('ratio_log2')}"
                      f"  target=0x{r['target_int']:064x}")
    print(f"=== fork blocks: {len(fork)}")
    for r in fork:
        print(f"    h{r['height']}  ts={r['ts']}  floor={r['target_int'] == FLOOR}")
    if waits:
        print(f"=== wait samples: {len(waits)}")
        for r in waits[-5:]:
            print(f"    {r}")
    for kind in ("freeze", "fork_built", "wait_end", "spontaneous_adoption",
                "forced_adoption", "produce_summary"):
        for r in rows():
            if r["kind"] == kind:
                print(f"=== {kind}: {json.dumps(r, indent=1)}")


if __name__ == "__main__":
    main()
