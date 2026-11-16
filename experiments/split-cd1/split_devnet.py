#!/usr/bin/env python3
"""split-cd1 release-node split demonstration.

Two stock release binaries (devnet-chaos/bin/parano1d) at different heads:
the honest chain (tip 7) and the fork chain (tip 24, whose block 6 carries the
CD-1 aliased coinbase slot). Node A full-syncs the honest chain, rejects the
fork's aliased block 6 at sequential validation, and stays at 7. Node B
snapshot-installs boundary 6 (the aliased block, terminal verified by the
release verifier) and suffix-syncs to 24.

Topology (HANDOFF section 4):
  S_H = stock parano1d + injected honest-chain dir (tip 7)
  S_F = stock parano1d + injected fork-chain dir   (tip 24)
  A   = stock parano1d, fresh, seeded [S_H] -> full-sync to 7;
        restart seeded [S_H, S_F] -> fork reject -> STAYS AT 7
  B   = stock parano1d, fresh, seeded [S_F] -> SNAPSHOT at boundary 6
        -> suffix-syncs to 24

Every process gets a PID file under run/devnet/ and is stopped only by that
PID (RPC paranoid_stop first). Loopback only. No pattern-based process
management.
"""

import json
import re
import shutil
import sys
import time
from pathlib import Path

sys.path.insert(0, "/Users/ember/dev/parano1d-break/experiments/devnet-chaos/scripts")
import chaoslib  # noqa: E402

LANE = Path(__file__).resolve().parent
RUN = LANE / "run"
DEVNET = RUN / "devnet"
LOGS = DEVNET / "logs"

# The stock release binaries stay where the devnet-chaos campaign keeps them;
# only the PID/log/run directories move to this lane.
chaoslib.RUN = DEVNET
chaoslib.LOGS = LOGS

P2P_BASE = 21120
RPC_BASE = 21121
PORT_STEP = 2

SETTLE = float(sys.argv[1]) if len(sys.argv) > 1 else 90.0


def node(name, seeds=()):
    index = {"s-h": 0, "s-f": 1, "a": 2, "b": 3}[name]
    return chaoslib.Node(
        name,
        P2P_BASE + index * PORT_STEP,
        RPC_BASE + index * PORT_STEP,
        mode="node",
        seeds=list(seeds),
        log="debug",
    )


def grep_log(node_obj, pattern, since_marker=None):
    lines = node_obj.log_lines_since(since_marker)
    rx = re.compile(pattern)
    return [line for line in lines if rx.search(line)]


def main():
    honest_src = RUN / "honest-chain"
    fork_src = RUN / "fork-chain"
    for path in (honest_src, fork_src, chaoslib.NODE_BIN):
        if not path.exists():
            raise SystemExit(f"missing {path}")
    if DEVNET.exists():
        raise SystemExit(f"{DEVNET} already exists; move it away for a fresh run")
    (DEVNET / "nodes").mkdir(parents=True)
    LOGS.mkdir(parents=True, exist_ok=True)

    s_h_dir = DEVNET / "nodes" / "s-h"
    s_f_dir = DEVNET / "nodes" / "s-f"
    shutil.copytree(honest_src, s_h_dir)
    shutil.copytree(fork_src, s_f_dir)

    summary = {
        "binaries": {
            "node": str(chaoslib.NODE_BIN),
        },
        "ports": {"p2p_base": P2P_BASE, "rpc_base": RPC_BASE},
    }

    # S_H and S_F: stock nodes opening the injected chain dirs. This is also
    # the MDBX Linux->macOS portability + DB-trust test.
    s_h = node("s-h")
    s_h.data_dir = s_h_dir
    s_h.start()
    s_f = node("s-f")
    s_f.data_dir = s_f_dir
    s_f.start()
    h_height, h_tip = s_h.tip()
    f_height, f_tip = s_f.tip()
    print(f"[evidence] S_H opened injected honest chain: height={h_height} tip={h_tip}")
    print(f"[evidence] S_F opened injected fork chain:   height={f_height} tip={f_tip}")
    if h_height != 7:
        raise SystemExit(f"S_H height {h_height} != 7")
    if f_height != 24:
        raise SystemExit(f"S_F height {f_height} != 24")
    summary["s_h"] = {"height": h_height, "tip": h_tip}
    summary["s_f"] = {"height": f_height, "tip": f_tip}

    # A: fresh node, seeded only with S_H. boundary(7) = 0, so it full-syncs.
    a = node("a", seeds=[f"127.0.0.1:{P2P_BASE}"])
    a.start()
    chaoslib.wait_until(
        "A full-syncs the honest chain to 7",
        lambda: a.height() == 7,
        timeout=600,
        interval=2.0,
    )
    a_height_1, a_tip_1 = a.tip()
    print(f"[evidence] A full-synced to {a_height_1} (tip {a_tip_1})")
    if a_tip_1 != h_tip:
        raise SystemExit(f"A tip {a_tip_1} != S_H tip {h_tip}")
    summary["a_after_first_sync"] = {"height": a_height_1, "tip": a_tip_1}

    # Restart A with both seeds: it must see the fork tip 24, fetch fork
    # block 6, reject it at sequential validation, and stay at 7.
    a.stop()
    a2 = node("a", seeds=[f"127.0.0.1:{P2P_BASE}", f"127.0.0.1:{P2P_BASE + PORT_STEP}"])
    a2.data_dir = a.data_dir
    a2.start()
    print(f"[wait] A sees the fork tip and settles ({SETTLE:.0f}s)", flush=True)
    time.sleep(SETTLE)
    a_height_2, a_tip_2 = a2.tip()
    reject_lines = grep_log(
        a2,
        "outside block log_slots|ShapeMismatch",
    )
    print(f"[evidence] A after fork exposure: height={a_height_2} tip={a_tip_2}")
    for line in reject_lines[-8:]:
        print(f"[a-reject] {line}")
    if a_height_2 != 7 or a_tip_2 != h_tip:
        raise SystemExit(f"A moved off the honest tip: {a_height_2} {a_tip_2}")
    if not reject_lines:
        raise SystemExit("A log shows no slot-domain ShapeMismatch rejection")
    summary["a_after_fork_exposure"] = {
        "height": a_height_2,
        "tip": a_tip_2,
        "reject_lines": reject_lines[-8:],
    }

    # B: fresh node, seeded only with S_F. boundary(24) = 6 > 0, so it
    # snapshot-installs boundary 6 (the aliased block) and suffix-syncs.
    b = node("b", seeds=[f"127.0.0.1:{P2P_BASE + PORT_STEP}"])
    b.start()
    chaoslib.wait_until(
        "B snapshot-installs and suffix-syncs the fork to 24",
        lambda: b.height() == 24,
        timeout=1800,
        interval=3.0,
    )
    b_height, b_tip = b.tip()
    print(f"[evidence] B at {b_height} (tip {b_tip})")
    if b_tip != f_tip:
        raise SystemExit(f"B tip {b_tip} != S_F tip {f_tip}")
    snapshot_lines = grep_log(
        b,
        "snapshot|Snapshot|terminal|Terminal|install",
    )
    for line in snapshot_lines[-20:]:
        print(f"[b-path] {line}")
    summary["b"] = {
        "height": b_height,
        "tip": b_tip,
        "path_lines": snapshot_lines[-20:],
    }

    # THE SPLIT: two release nodes, different heads, both serving.
    final_a_height, final_a_tip = a2.tip()
    final_b_height, final_b_tip = b.tip()
    split = (
        final_a_height == 7
        and final_a_tip == h_tip
        and final_b_height == 24
        and final_b_tip == f_tip
        and final_a_tip != final_b_tip
    )
    summary["split"] = {
        "observed": split,
        "a": {"height": final_a_height, "tip": final_a_tip},
        "b": {"height": final_b_height, "tip": final_b_tip},
    }
    print(
        f"[result] A at {final_a_height} ({final_a_tip[:16]}...) vs "
        f"B at {final_b_height} ({final_b_tip[:16]}...): "
        + ("SPLIT OBSERVED" if split else "NO SPLIT")
    )

    summary_path = DEVNET / "split-summary.json"
    summary_path.write_text(json.dumps(summary, indent=2) + "\n")
    print(f"[evidence] summary written to {summary_path}")

    for n in (a2, b, s_h, s_f):
        n.stop()
    print("[done] all nodes stopped by PID")
    return 0 if split else 1


if __name__ == "__main__":
    raise SystemExit(main())
