#!/usr/bin/env python3
"""Measurement sampler for the forge-bench lane (runs on hbox).

Every 5 s, one JSON line appended to logs/samples.jsonl:
  - node: getChainInfo (height, best_hash, difficulty_target)
  - forge: GET /summary on the miner API (127.0.0.1:7777)
  - GPU sysfs: power (W), temps (C), fan (rpm), clocks (MHz) from the
    RX 6700 XT (drm card2, hwmon4)

Read-only everywhere. Stop by PID file.
"""

import glob
import json
import sys
import time
import urllib.request

RPC = "http://127.0.0.1:21241"
FORGE_SUMMARY = "http://127.0.0.1:7777/summary"
OUT = "/home/hbox/lanes/forge-bench/logs/samples.jsonl"
HWMON_GLOB = "/sys/class/drm/card2/device/hwmon/hwmon*"
INTERVAL = 5.0


def rpc(method, params=None):
    body = json.dumps(
        {"jsonrpc": "2.0", "id": 1, "method": f"paranoid_{method}", "params": params or []}
    ).encode()
    req = urllib.request.Request(RPC, data=body, headers={"content-type": "application/json"})
    with urllib.request.urlopen(req, timeout=10) as resp:
        return json.loads(resp.read().decode())["result"]


def read_int(path):
    try:
        with open(path) as handle:
            return int(handle.read().strip())
    except (OSError, ValueError):
        return None


def gpu_sysfs():
    record = {}
    hwmons = sorted(glob.glob(HWMON_GLOB))
    if not hwmons:
        return {"error": "no hwmon"}
    base = hwmons[0]
    power_uw = read_int(f"{base}/power1_average")
    record["power_w"] = power_uw / 1e6 if power_uw is not None else None
    record["power_cap_w"] = (read_int(f"{base}/power1_cap") or 0) / 1e6
    for idx in (1, 2, 3):
        milli = read_int(f"{base}/temp{idx}_input")
        if milli is not None:
            record[f"temp{idx}_c"] = milli / 1e3
    fan = read_int(f"{base}/fan1_input")
    if fan is not None:
        record["fan_rpm"] = fan
    for idx in (1, 2):
        hz = read_int(f"{base}/freq{idx}_input")
        if hz is not None:
            record[f"freq{idx}_mhz"] = hz / 1e6
    return record


def sample():
    row = {"ts": time.time(), "iso": time.strftime("%Y-%m-%dT%H:%M:%S%z")}
    try:
        row["chain"] = rpc("getChainInfo")
    except Exception as exc:  # noqa: BLE001 - sampler keeps going on errors
        row["chain_error"] = repr(exc)
    try:
        with urllib.request.urlopen(FORGE_SUMMARY, timeout=10) as resp:
            row["forge"] = json.loads(resp.read().decode())
    except Exception as exc:  # noqa: BLE001
        row["forge_error"] = repr(exc)
    row["gpu"] = gpu_sysfs()
    return row


def main():
    interval = float(sys.argv[1]) if len(sys.argv) > 1 else INTERVAL
    with open(OUT, "a", encoding="utf-8") as out:
        while True:
            row = sample()
            out.write(json.dumps(row) + "\n")
            out.flush()
            time.sleep(interval)


if __name__ == "__main__":
    main()
