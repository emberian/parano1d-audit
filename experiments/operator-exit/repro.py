#!/usr/bin/env python3
"""Loopback-only wallet acknowledgment vs independent peer observation."""
from pathlib import Path
import json
import os
import signal
import subprocess
import time
import urllib.request
import re

ROOT = Path(__file__).resolve().parents[2]
LANE = Path(__file__).resolve().parent
RUN = LANE / "run"
SRC = ROOT / "experiments/devnet-chaos/run"
BIN = ROOT / "experiments/devnet-chaos/bin"
NODE = BIN / "parano1d"
CLI = BIN / "parano1d-cli"
RUN_ID = os.environ.get("OPERATOR_EXIT_RUN_ID", time.strftime("%Y%m%d-%H%M%S"))
if not re.fullmatch(r"[A-Za-z0-9_-]{1,48}", RUN_ID):
    raise RuntimeError("OPERATOR_EXIT_RUN_ID must be 1-48 letters, numbers, _ or -")
RUN.mkdir(exist_ok=True)


def call(port, method, params=()):
    request = urllib.request.Request(
        f"http://127.0.0.1:{port}",
        data=json.dumps({"jsonrpc": "2.0", "id": 1, "method": f"paranoid_{method}", "params": params}).encode(),
        headers={"content-type": "application/json"},
    )
    with urllib.request.urlopen(request, timeout=15) as response:
        body = json.load(response)
    if "error" in body:
        raise RuntimeError(body["error"])
    return body["result"]


def wait(name, fn, timeout=60):
    end = time.time() + timeout
    last = None
    while time.time() < end:
        try:
            value = fn()
            if value:
                return value
            last = value
        except Exception as exc:
            last = repr(exc)
        time.sleep(1)
    raise RuntimeError(f"timeout {name}: {last}")


def clone(source, name):
    target = RUN / name
    if target.exists():
        raise RuntimeError(f"refusing to overwrite {target}")
    subprocess.run(["cp", "-cR", str(SRC / source), str(target)], check=True)
    # Distinct P2P identities; the chain data and synthetic funded wallet remain.
    (target / "p2p_identity.key").unlink(missing_ok=True)
    return target


def start(name, path, p2p, rpc, seeds=()):
    cmd = [str(NODE), "--mode", "node", "--data-dir", str(path),
           "--p2p-listen", f"127.0.0.1:{p2p}", "--rpc-listen", f"127.0.0.1:{rpc}",
           "--disable-dns-seeds", "--log", "info"]
    for seed in seeds:
        cmd.extend(["--seed", seed])
    stream = open(RUN / f"{name}.log", "ab", buffering=0)
    process = subprocess.Popen(cmd, stdout=stream, stderr=subprocess.STDOUT)
    (RUN / f"{name}.pid").write_text(str(process.pid))
    wait(f"{name} RPC", lambda: call(rpc, "getChainInfo"), timeout=90)
    return process, stream


def stop(name, process, stream, rpc):
    if process.poll() is None:
        try:
            call(rpc, "stop")
        except Exception:
            pass
        try:
            process.wait(timeout=15)
        except subprocess.TimeoutExpired:
            process.send_signal(signal.SIGTERM)
            process.wait(timeout=10)
    stream.close()
    (RUN / f"{name}.pid").unlink(missing_ok=True)


def address(port):
    result = subprocess.run([str(CLI), "--rpc", f"http://127.0.0.1:{port}", "address"],
                            capture_output=True, text=True, check=True)
    return next(word for word in result.stdout.split() if word.startswith("o1"))


sender = receiver = None
try:
    suffix = "-host-" + RUN_ID
    sender_path = clone("n1-pool", "sender" + suffix)
    receiver_path = clone("n2-relay", "receiver" + suffix)
    sender = start("sender", sender_path, 19910, 19911)
    receiver = start("receiver", receiver_path, 19920, 19921)
    receiver_address = address(19921)
    sender_peers_before = call(19911, "getPeerCount")
    receiver_peers_before = call(19921, "getPeerCount")
    assert sender_peers_before == receiver_peers_before == 0, "test nodes unexpectedly connected"
    before = call(19921, "getMempoolSize")
    # 0 fee asks the wallet to choose its own current admission fee.
    result = call(19911, "walletSend", [receiver_address, 100_000, 0])
    txid = result["txid"]
    time.sleep(5)
    sender_peers_after = call(19911, "getPeerCount")
    receiver_peers_after = call(19921, "getPeerCount")
    assert sender_peers_after == receiver_peers_after == 0, "peer appeared during partition control"
    unseen = call(19921, "getMempoolEntry", [txid])
    print(json.dumps({"sender_reply": result, "receiver_pool_before": before,
                      "run_id": RUN_ID,
                      "peers_before": [sender_peers_before, receiver_peers_before],
                      "peers_after": [sender_peers_after, receiver_peers_after],
                      "receiver_entry_after_5s_is_none": unseen is None,
                      "sender_tip": call(19911, "getChainInfo")["height"],
                      "receiver_tip": call(19921, "getChainInfo")["height"]}, indent=2))
    assert unseen is None
    stop("receiver", *receiver, 19921)
    receiver = start("receiver", receiver_path, 19920, 19921,
                     seeds=["127.0.0.1:19910"])
    entry = wait("receiver mempool entry after peer connection",
                 lambda: call(19921, "getMempoolEntry", [txid]), timeout=120)
    print(json.dumps({"receiver_observed_after_connection": entry is not None,
                      "receiver_peers_after_connection": call(19921, "getPeerCount"),
                      "txid": txid}, indent=2))
finally:
    if receiver:
        stop("receiver", *receiver, 19921)
    if sender:
        stop("sender", *sender, 19911)
