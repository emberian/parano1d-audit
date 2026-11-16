#!/usr/bin/env python3
"""Query the saved synthetic fixture through an isolated loopback JSON-RPC node."""

from pathlib import Path
import json
import re
import signal
import statistics
import subprocess
import time
import urllib.request

lane = Path(__file__).resolve().parent
root = lane.parents[1]
temporary = lane / "tmp/page-timing"
prior = (lane / "page-timing-results.txt").read_text()
fixture = re.search(r"^FIXTURE_DB=(.*)$", prior, re.MULTILINE).group(1)
victim = re.search(r"^FIXTURE_VICTIM=(.*)$", prior, re.MULTILINE).group(1)
empty = re.search(r"^FIXTURE_EMPTY=(.*)$", prior, re.MULTILINE).group(1)
rpc_url = "http://127.0.0.1:19941"

def rpc(method, params):
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
    request = urllib.request.Request(rpc_url, data=body, headers={"Content-Type": "application/json"})
    started = time.perf_counter_ns()
    with urllib.request.urlopen(request, timeout=10) as response:
        wire = response.read()
    elapsed_us = (time.perf_counter_ns() - started) // 1000
    decoded = json.loads(wire)
    if "error" in decoded:
        raise RuntimeError(f"{method}: {decoded['error']}")
    return decoded["result"], len(wire), elapsed_us

def summary(values):
    ordered = sorted(values)
    return f"p50={ordered[len(ordered)//2]}us p95={ordered[len(ordered)*95//100]}us min={ordered[0]}us max={ordered[-1]}us"

daemon = root / "experiments/devnet-chaos/bin/parano1d"
log_path = temporary / "rpc-node-default.log"
with log_path.open("w") as log:
    node = subprocess.Popen([
        str(daemon), "-c", str(temporary / "rpc-node-default.toml"),
        "--mode", "node", "--genesis", "--data-dir", fixture,
        "--p2p-listen", "127.0.0.1:19940", "--rpc-listen", "127.0.0.1:19941",
        "--disable-dns-seeds", "--log", "warn",
    ], stdout=log, stderr=subprocess.STDOUT)
    try:
        for _ in range(60):
            if node.poll() is not None:
                raise RuntimeError(f"local node exited {node.returncode}; see {log_path}")
            try:
                info, _, _ = rpc("getChainInfo", [])
                break
            except (OSError, RuntimeError):
                time.sleep(1)
        else:
            raise RuntimeError(f"local RPC did not start; see {log_path}")
        print(f"CONTROL ordinary local node RPC tip height={info['height']} (fixture expected 26)")
        assert info["height"] == 26
        page_us = []
        scan_us = []
        scan_bytes = []
        for _ in range(100):
            cursor = None
            count = 0
            total_wire = 0
            started_scan = time.perf_counter_ns()
            while True:
                page, wire_len, elapsed_us = rpc("getNativeHoldings", [victim, cursor, 64])
                page_us.append(elapsed_us)
                total_wire += wire_len
                count += len(page["holdings"])
                cursor = page["next"]
                if cursor is None:
                    break
            assert count == 248
            scan_us.append((time.perf_counter_ns() - started_scan) // 1000)
            scan_bytes.append(total_wire)
        empty_us = []
        for _ in range(100):
            page, _, elapsed_us = rpc("getNativeHoldings", [empty, None, 64])
            assert not page["holdings"]
            empty_us.append(elapsed_us)
        output = (
            f"MEASURE ordinary local loopback JSON-RPC page (64 items; n={len(page_us)}): {summary(page_us)}\n"
            f"MEASURE ordinary local loopback JSON-RPC full scan (248 items; n={len(scan_us)}): {summary(scan_us)}\n"
            f"MEASURE ordinary local loopback JSON-RPC full scan response bytes: p50={int(statistics.median(scan_bytes))}\n"
            f"CONTROL ordinary local loopback JSON-RPC empty page (n={len(empty_us)}): {summary(empty_us)}\n"
        )
        print(output, end="", flush=True)
        (lane / "rpc-timing-results.txt").write_text(output)
    finally:
        node.send_signal(signal.SIGTERM)
        try:
            node.wait(timeout=15)
        except subprocess.TimeoutExpired:
            node.kill()
            node.wait()
