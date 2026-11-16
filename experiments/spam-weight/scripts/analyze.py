#!/usr/bin/env python3
"""Summarize run/blocks.csv into the report's measured table.

Usage: python3 scripts/analyze.py [run/blocks.csv]
"""

import csv
import statistics
import sys
from pathlib import Path

LANE = Path(__file__).resolve().parents[1]


def load(path):
    rows = []
    with open(path, newline="") as handle:
        for row in csv.DictReader(handle):
            rows.append(row)
    return rows


def fnum(row, key):
    try:
        return float(row[key])
    except (ValueError, TypeError, KeyError):
        return None


def window_stats(rows, phase):
    rows = [r for r in rows if r["phase"] == phase]
    if not rows:
        return None
    out = {"n": len(rows), "first_h": int(rows[0]["height"]), "last_h": int(rows[-1]["height"])}
    for key in (
        "block_secs", "user_pages", "live_outputs", "live_inputs", "logical_txs",
        "block_bytes", "history_step_bytes",
    ):
        vals = [fnum(r, key) for r in rows]
        vals = [v for v in vals if v is not None]
        out[f"mean_{key}"] = statistics.mean(vals) if vals else None
    fees = [fnum(r, "total_fees") for r in rows]
    fees = [v for v in fees if v is not None]
    out["mean_fees"] = statistics.mean(fees) if fees else None
    rewards = [fnum(r, "reward") for r in rows]
    rewards = [v for v in rewards if v is not None]
    out["mean_reward"] = statistics.mean(rewards) if rewards else None
    sb = [fnum(r, "state_bytes") for r in rows]
    sb = [v for v in sb if v is not None]
    if sb:
        out["state_bytes_first"], out["state_bytes_last"] = sb[0], sb[-1]
        out["state_growth_total"] = sb[-1] - sb[0]
    du1 = [fnum(r, "n1_dir_kb") for r in rows]
    du1 = [v for v in du1 if v is not None and v >= 0]
    if du1:
        out["n1_dir_first"], out["n1_dir_last"] = du1[0], du1[-1]
        out["n1_dir_growth_kb"] = du1[-1] - du1[0]
    du2 = [fnum(r, "n2_dir_kb") for r in rows]
    du2 = [v for v in du2 if v is not None and v >= 0]
    if du2:
        out["n2_dir_first"], out["n2_dir_last"] = du2[0], du2[-1]
        out["n2_dir_growth_kb"] = du2[-1] - du2[0]
    slots = [fnum(r, "active_slots") for r in rows]
    slots = [v for v in slots if v is not None]
    if slots:
        out["slots_first"], out["slots_last"] = slots[0], slots[-1]
    submits = [r.get("submit", "") for r in rows]
    admitted = sum(1 for s in submits if s and "R" not in s and "+" in s)
    out["rounds_both_admitted"] = admitted
    return out


def main():
    path = sys.argv[1] if len(sys.argv) > 1 else str(LANE / "run" / "blocks.csv")
    rows = load(path)
    control = window_stats(rows, "control")
    spam = window_stats(rows, "spam")
    post = window_stats(rows, "post")

    print("=== windows ===")
    for name, stats in (("control", control), ("spam", spam), ("post", post)):
        if stats:
            print(name, {k: v for k, v in stats.items()})

    if control and spam:
        overhead = control["mean_live_outputs"] or 0
        items_per_block = (spam["mean_live_outputs"] or 0) - overhead
        slots_per_block = (spam["slots_last"] or 0) - (spam["slots_first"] or 0)
        subsidy = control["mean_reward"] or 0
        fees = spam["mean_fees"] or 0
        reward = spam["mean_reward"] or 0
        recaptured = reward - subsidy
        burn = fees - recaptured
        blocks_per_day_30s = 2880
        cadence = spam["mean_block_secs"] or 30
        blocks_per_day = 86400 / cadence if cadence else blocks_per_day_30s
        items_day = items_per_block * blocks_per_day
        state_day = (spam.get("state_growth_total") or 0) / spam["n"] * blocks_per_day
        dir_day = (spam.get("n1_dir_growth_kb") or 0) / spam["n"] * blocks_per_day
        burn_day = burn * blocks_per_day
        noid_per_mib_state = burn_day / (state_day / 1_048_576) if state_day else None
        print("=== derived ===")
        print(f"items/block (spam, net of {overhead:.1f} overhead): {items_per_block:.1f}")
        print(f"net slots/block: {slots_per_block / spam['n']:.1f}")
        print(f"mean fees/block: {fees:,.0f} uNOID; reward {reward:,.0f} (subsidy {subsidy:,.0f})")
        print(f"recaptured/block: {recaptured:,.0f} uNOID; burned/block: {burn:,.0f} uNOID")
        print(f"cadence: {cadence:.1f}s -> {blocks_per_day:.0f} blocks/day")
        print(f"items/day: {items_day:,.0f}")
        print(f"sparse state growth/day: {state_day/1_048_576:.2f} MiB")
        print(f"n1 dir growth/day: {dir_day/1024:.2f} MiB (MDBX file granularity)")
        print(f"burned/day: {burn_day/1_000_000:.2f} NOID")
        if noid_per_mib_state:
            print(f"NOID per MiB permanent sparse state: {noid_per_mib_state:.2f}")
        if state_day:
            per_item_state = (spam.get("state_growth_total") or 0) / max(1, items_per_block * spam["n"])
            print(f"sparse bytes per item: {per_item_state:.1f}")
        items_total = items_per_block * spam["n"]
        if items_total:
            print(f"total items minted: {items_total:,.0f}")
            per_item_burn = burn / items_per_block if items_per_block else 0
            print(f"uNOID burned per item: {per_item_burn:,.0f}")
            logical_per_item = 491  # prior-art MDBX logical bytes per item
            print(
                f"NOID per GB node-local (logical {logical_per_item} B/item): "
                f"{per_item_burn * 1_073_741_824 / (logical_per_item * 1_000_000):.2f}"
            )


if __name__ == "__main__":
    main()
