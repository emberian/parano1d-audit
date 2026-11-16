#!/usr/bin/env python3
"""Read-only verification of the chaos-asert run against raw node state.

Queries paranoid_getBlockHeader / paranoid_getChainInfo over loopback RPC and
checks the collapse claims block by block. Never writes anything.
"""
import json
import sys
import urllib.request

FLOOR = "00" * 29 + "40" + "00" * 2  # 2^238, little-endian target hex
FLOOR_INT = 1 << 238
P = {"n1": 21101, "n2": 21103, "P": 21105, "S": 21107}


def rpc(port, method, params=None, timeout=30):
    body = json.dumps(
        {"jsonrpc": "2.0", "id": 1, "method": f"paranoid_{method}", "params": params or []}
    ).encode()
    req = urllib.request.Request(
        f"http://127.0.0.1:{port}", data=body, headers={"content-type": "application/json"}
    )
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read().decode()).get("result")


def tips():
    out = {}
    for name, port in P.items():
        try:
            i = rpc(port, "getChainInfo")
            out[name] = (i.get("height"), str(i.get("best_hash"))[:16])
        except Exception as e:  # noqa: BLE001
            out[name] = ("ERR", str(e)[:60])
    return out


def check_fork(lo, hi, port=21105):
    """Every fork block in [lo, hi] must carry the exact genesis floor target."""
    bad = []
    rows = []
    for h in range(lo, hi + 1):
        hdr = rpc(port, "getBlockHeader", [h])
        if not isinstance(hdr, dict):
            print(f"h{h}: MISSING ({hdr})")
            bad.append(h)
            continue
        t = hdr.get("difficulty_target")
        ts = hdr.get("timestamp")
        ok = t == FLOOR
        if not ok:
            bad.append(h)
        rows.append((h, ts, "FLOOR" if ok else t, str(hdr.get("hash"))[:12]))
    for h, ts, t, hh in rows:
        print(f"h{h}: ts={ts} target={t} hash={hh}")
    print("NON-FLOOR HEIGHTS:", bad or "none — all at exact 2^238 floor")
    return bad


if __name__ == "__main__":
    what = sys.argv[1] if len(sys.argv) > 1 else "tips"
    if what == "tips":
        for name, t in tips().items():
            print(name, "tip:", t[0], t[1])
    elif what == "fork":
        lo, hi = int(sys.argv[2]), int(sys.argv[3])
        port = int(sys.argv[4]) if len(sys.argv) > 4 else 21105
        check_fork(lo, hi, port)
    elif what == "anchor":
        # anchor h42 ratio check straight from the header
        hdr = rpc(21101, "getBlockHeader", [42])
        t = hdr.get("difficulty_target")
        ti = int.from_bytes(bytes.fromhex(t), "little")
        print("h42 anchor target:", t)
        print("ratio R = FLOOR/anchor =", round(FLOOR_INT / ti, 3),
              " ratio_log2 =", round(__import__("math").log2(FLOOR_INT / ti), 3))
