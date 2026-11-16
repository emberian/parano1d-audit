#!/usr/bin/env python3
"""Isolated release-verifier test for one future-dated historical header.

Uses an existing locally built poison miner. All data, logs, and PID files are
under this lane. No DNS seeds and no public listeners. A run gets a new
timestamped directory; existing runs are never reused or removed.
"""

import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import time
import urllib.request

LANE = Path(__file__).resolve().parent
ROOT = LANE.parents[1]
POISON_BIN = ROOT / "experiments/devnet-chaos/bin/parano1d-poison"
VICTIM_BIN = ROOT / "experiments/devnet-chaos/bin/parano1d"
CLI_BIN = ROOT / "experiments/devnet-chaos/bin/parano1d-cli"
POISON_RPC = "http://127.0.0.1:19941"
VICTIM_RPC = "http://127.0.0.1:19951"
FUTURE_SECS = 10 * 365 * 24 * 3600
POISON_HEIGHT = 11  # release v1 ASERT anchor is h12, not h11


def rpc(url, method, timeout=3):
    body = json.dumps({"jsonrpc": "2.0", "id": 1,
                       "method": f"paranoid_{method}", "params": []}).encode()
    req = urllib.request.Request(url, data=body,
                                 headers={"content-type": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as response:
        payload = json.load(response)
    if "error" in payload:
        raise RuntimeError(payload["error"])
    return payload["result"]


def info(url):
    return rpc(url, "getChainInfo")


def wait_height(url, minimum, timeout, process):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"node exited early with {process.returncode}")
        try:
            value = info(url)
            if value["height"] >= minimum:
                return value
        except (OSError, KeyError, ValueError, RuntimeError):
            pass
        time.sleep(0.25)
    raise TimeoutError(f"node did not reach h{minimum} in {timeout}s")


def block_header(url, height):
    proc = subprocess.run([str(CLI_BIN), "--rpc", url, "block-header", str(height)],
                          text=True, capture_output=True, timeout=20, check=True)
    match = re.search(r"[Tt]imestamp\D+(\d{9,})", proc.stdout)
    return int(match[1]) if match else proc.stdout.strip()


class Node:
    def __init__(self, name, binary, args, run_dir, offset=None):
        self.name = name
        self.binary = binary
        self.args = args
        self.run_dir = run_dir
        self.offset = offset
        self.proc = None
        self.log = None
        self.pidfile = run_dir / f"{name}.pid"

    def start(self):
        if self.pidfile.exists():
            raise RuntimeError(f"refusing existing PID file {self.pidfile}")
        self.log = open(self.run_dir / f"{self.name}.log", "ab", buffering=0)
        env = os.environ.copy()
        if self.offset is not None:
            env["NOID_POISON_CLOCK_OFFSET_SECS"] = str(self.offset)
        else:
            env.pop("NOID_POISON_CLOCK_OFFSET_SECS", None)
        self.proc = subprocess.Popen([str(self.binary), *self.args],
                                     stdout=self.log, stderr=subprocess.STDOUT, env=env)
        self.pidfile.write_text(str(self.proc.pid))
        print(f"start {self.name} pid={self.proc.pid} offset={self.offset}", flush=True)
        return self

    def stop(self, url):
        if self.proc is None:
            return
        try:
            rpc(url, "stop")
        except Exception:
            pass
        try:
            self.proc.wait(timeout=15)
        except subprocess.TimeoutExpired:
            self.proc.send_signal(signal.SIGTERM)
            try:
                self.proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait(timeout=5)
        print(f"stop {self.name} exit={self.proc.returncode}", flush=True)
        self.pidfile.unlink(missing_ok=True)
        self.log.close()
        self.proc = None


def main():
    for binary in (POISON_BIN, VICTIM_BIN, CLI_BIN):
        if not binary.is_file():
            raise RuntimeError(f"missing existing build {binary}")
    case = LANE / "run" / time.strftime("middle-h11-%Y%m%d-%H%M%S")
    case.mkdir(parents=True, exist_ok=False)
    print(f"case={case}", flush=True)
    poison_data = case / "poison-data"
    victim_data = case / "victim-data"
    poison_args = ["--mode", "miner", "--genesis", "--data-dir", str(poison_data),
                   "--p2p-listen", "127.0.0.1:19940", "--rpc-listen", "127.0.0.1:19941",
                   "--disable-dns-seeds", "--log", "debug"]
    victim_args = ["--mode", "node", "--data-dir", str(victim_data),
                   "--p2p-listen", "127.0.0.1:19950", "--rpc-listen", "127.0.0.1:19951",
                   "--seed", "127.0.0.1:19940", "--disable-dns-seeds", "--log", "debug"]
    active = []

    def stage(label, offset, target, timeout):
        node = Node(label, POISON_BIN, poison_args, case, offset).start()
        active.append((node, POISON_RPC))
        result = wait_height(POISON_RPC, target, timeout, node.proc)
        print(f"{label} height={result['height']} tip={result['best_hash']}", flush=True)
        node.stop(POISON_RPC)
        active.remove((node, POISON_RPC))
        if result["height"] != target:
            raise RuntimeError(f"overshot {label}: wanted h{target}, got h{result['height']}")
        return result

    try:
        stage("normal-prefix", None, POISON_HEIGHT - 1, 360)
        stage("future-outlier", FUTURE_SECS, POISON_HEIGHT, 90)
        # A failed restart or proof for h12 is a decisive fixture barrier.
        node = Node("normal-suffix", POISON_BIN, poison_args, case).start()
        active.append((node, POISON_RPC))
        try:
            h12 = wait_height(POISON_RPC, POISON_HEIGHT + 1, 120, node.proc)
            print(f"normal-suffix-restarted h{h12['height']}", flush=True)
            for height in (10, 11, 12):
                print(f"poison-header h{height} timestamp={block_header(POISON_RPC, height)}", flush=True)
            # h36 gives a finalized snapshot boundary at h18; h42 gives h24.
            tip = wait_height(POISON_RPC, 42, 600, node.proc)
            print(f"poison-tip h{tip['height']} hash={tip['best_hash']}"
                  f" timestamp={block_header(POISON_RPC, tip['height'])}", flush=True)
        finally:
            node.stop(POISON_RPC)
            active.remove((node, POISON_RPC))

        # Serving node stays online but stops mining; this freezes its offer.
        serve_args = poison_args.copy()
        serve_args[serve_args.index("miner")] = "node"
        serve_args.remove("--genesis")
        server = Node("poison-serving", POISON_BIN, serve_args, case).start()
        active.append((server, POISON_RPC))
        victim = Node("fresh-victim", VICTIM_BIN, victim_args, case).start()
        active.append((victim, VICTIM_RPC))
        samples = []
        deadline = time.monotonic() + 240
        while time.monotonic() < deadline:
            if victim.proc.poll() is not None:
                break
            try:
                observed = info(VICTIM_RPC)
                samples.append((observed["height"], observed["best_hash"]))
                if observed["height"] >= 18:
                    break
            except Exception:
                pass
            time.sleep(2)
        print(f"victim-samples={samples}", flush=True)
        print(f"victim-final={info(VICTIM_RPC)}", flush=True)
        print("review both logs for terminal verdict, snapshot installation, and finality", flush=True)
    finally:
        for node, url in reversed(active):
            node.stop(url)


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(f"FAIL: {error}", file=sys.stderr, flush=True)
        raise
