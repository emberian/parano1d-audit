#!/usr/bin/env python3
"""Analyzer for the forge-bench run (runs on the Mac against pulled logs).

Inputs (scp'd from hbox):
  logs/samples.jsonl  - sampler rows: ts, chain{height,difficulty_target}, forge summary, gpu
  logs/headers.jsonl  - per-block headers: height, timestamp, miner, difficulty_target

Outputs (stdout):
  1. floor window: blocks found while target == genesis floor, rate per minute,
     per-block spacing (node-proving-bound observation)
  2. steady-state window (difficulty converged, last 20 min of the run):
     forge-reported hashrate (median + IQR of /summary totals)
     implied hashrate from chain work: sum(2^256/target)/dt
     block spacing vs the 30 s target
  3. GPU power draw + efficiency during the steady window
"""

import json
import math
import statistics
import sys

GENESIS_FLOOR_TARGET = bytes.fromhex(
    "00" * 28 + "00400000"
)  # 2^238 LE; byte 29 = 0x40


def le256_to_int(hexstr: str) -> int:
    raw = bytes.fromhex(hexstr)
    return int.from_bytes(raw, "little")


def work(target_int: int) -> float:
    return 2**256 / target_int  # expected hashes per block


def load_jsonl(path):
    with open(path, encoding="utf-8") as handle:
        return [json.loads(line) for line in handle if line.strip()]


def main():
    samples_path, headers_path = sys.argv[1], sys.argv[2]
    samples = load_jsonl(samples_path)
    headers = load_jsonl(headers_path)

    print(f"samples: {len(samples)}  headers: {len(headers)}")

    # ---- forge-reported hashrate over time ----
    forge_rates = []
    for row in samples:
        forge = row.get("forge")
        if not forge:
            continue
        # /summary schema is recorded as-is; find the total rate field
        total = None
        if isinstance(forge, dict):
            for key in ("total_hashrate", "hashrate_total", "total", "hashrate"):
                if key in forge:
                    total = forge[key]
                    break
            if total is None and "gpus" in forge and forge["gpus"]:
                # sum per-gpu rates if the schema nests them
                gpus = forge["gpus"]
                if isinstance(gpus, list) and gpus and isinstance(gpus[0], dict):
                    rates = [
                        g.get("hashrate")
                        for g in gpus
                        if isinstance(g, dict) and g.get("hashrate") is not None
                    ]
                    if rates:
                        total = sum(rates)
        if total is not None:
            try:
                forge_rates.append((row["ts"], float(total)))
            except (TypeError, ValueError):
                pass

    print(f"forge rate samples: {len(forge_rates)}")
    if forge_rates:
        print("first forge rate row keys example:")
        first_forge = next((r["forge"] for r in samples if r.get("forge")), {})
        print(json.dumps(first_forge, indent=1)[:800])

    # ---- floor window from headers ----
    floor_blocks = [h for h in headers if h["difficulty_target"].endswith("00400000")]
    print(f"\nblocks at genesis-floor target: {len(floor_blocks)}")
    if len(floor_blocks) >= 2:
        times = [h["timestamp"] for h in floor_blocks]
        span = times[-1] - times[0]
        print(f"floor window span: {span} s for {len(floor_blocks)} blocks")
        if span > 0:
            print(f"floor block rate: {len(floor_blocks) / span * 60:.2f} blocks/min")
        spacings = [
            b["timestamp"] - a["timestamp"] for a, b in zip(floor_blocks, floor_blocks[1:])
        ]
        print(f"floor median spacing: {statistics.median(spacings):.2f} s")

    # ---- steady-state window: last 20 min of chain activity ----
    if headers:
        t_end = headers[-1]["timestamp"]
        t_start = t_end - 20 * 60
        window = [h for h in headers if h["timestamp"] >= t_start and h["height"] > 0]
        if len(window) >= 2:
            span = window[-1]["timestamp"] - window[0]["timestamp"]
            total_work = sum(work(le256_to_int(h["difficulty_target"])) for h in window[1:])
            implied = total_work / span if span > 0 else 0
            blocks = len(window) - 1
            print(f"\nsteady window: {blocks} blocks in {span:.0f} s "
                  f"({blocks / span * 60:.2f} blocks/min)")
            print(f"implied hashrate from chain work: {implied/1e6:.2f} MH/s "
                  f"({implied:.3e} H/s)")
            targets = [le256_to_int(h["difficulty_target"]) for h in window]
            print(f"converged target (median): 2^{math.log2(statistics.median(targets)):.1f}")
            spacings = [
                b["timestamp"] - a["timestamp"] for a, b in zip(window, window[1:])
            ]
            print(f"median block spacing: {statistics.median(spacings):.1f} s (30 s target)")
            miners = {h["miner"] for h in window}
            print(f"miners in window: {miners}")

    # ---- forge-reported steady rate ----
    if forge_rates:
        t_end_f = forge_rates[-1][0]
        steady = [r for ts, r in forge_rates if ts >= t_end_f - 20 * 60]
        if steady:
            med = statistics.median(steady)
            print(f"\nforge-reported rate (last 20 min): median {med:.1f}, "
                  f"mean {statistics.mean(steady):.1f}, "
                  f"p10 {sorted(steady)[len(steady)//10]:.1f}, "
                  f"p90 {sorted(steady)[(len(steady)*9)//10]:.1f}")

    # ---- power ----
    powers, temps = [], []
    t_end_s = samples[-1]["ts"] if samples else 0
    for row in samples:
        if row["ts"] >= t_end_s - 20 * 60:
            gpu = row.get("gpu") or {}
            if gpu.get("power_w") is not None:
                powers.append(gpu["power_w"])
            if gpu.get("temp1_c") is not None:
                temps.append(gpu["temp1_c"])
    if powers:
        print(f"\nGPU power (last 20 min): median {statistics.median(powers):.1f} W, "
              f"mean {statistics.mean(powers):.1f} W")
        print(f"GPU core temp: median {statistics.median(temps):.1f} C")


if __name__ == "__main__":
    main()
