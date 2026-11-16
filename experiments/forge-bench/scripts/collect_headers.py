#!/usr/bin/env python3
"""Post-run collector for the forge-bench lane (runs on hbox).

Walks every block header 0..tip over loopback RPC and writes one JSON line
per block to logs/headers.jsonl: height, hash, prev, timestamp, miner,
difficulty_target, nonce. This gives the per-block work series for the
implied-hashrate cross-check and the floor-difficulty window analysis.
"""

import json
import time
import urllib.request

RPC = "http://127.0.0.1:21241"
OUT = "/home/hbox/lanes/forge-bench/logs/headers.jsonl"


def rpc(method, params=None):
    body = json.dumps(
        {"jsonrpc": "2.0", "id": 1, "method": f"paranoid_{method}", "params": params or []}
    ).encode()
    req = urllib.request.Request(RPC, data=body, headers={"content-type": "application/json"})
    with urllib.request.urlopen(req, timeout=30) as resp:
        return json.loads(resp.read().decode())["result"]


def main():
    tip = rpc("getChainInfo")["height"]
    print(f"tip height {tip}")
    with open(OUT, "w", encoding="utf-8") as out:
        for height in range(0, tip + 1):
            header = rpc("getBlockHeader", [height])
            if header is None:
                print(f"height {height}: missing")
                continue
            out.write(json.dumps(header) + "\n")
            if height % 100 == 0:
                print(f"...{height}")
                out.flush()
    print("done")


if __name__ == "__main__":
    main()
