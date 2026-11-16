#!/usr/bin/env python3
"""Read-only arithmetic audit of comp-sync-finality's saved ASERT result.

This script does not hash headers, mine blocks, open a node, or use a network.
It deliberately treats the source JSON as a *claim* to check, not chain data.
"""

import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SOURCE = ROOT / "experiments/comp-sync-finality/logs/asert_mtp_conformance.json"
OUT = Path(__file__).with_name("results.json")
U256 = 1 << 256
FLOOR_TARGET = 1 << 238
FINALITY_DEPTH = 18


def trunc_div(n: int, d: int) -> int:
    return n // d if n >= 0 else -((-n) // d)


def bch_factor(frac: int) -> int:
    f = frac
    return 65536 + (
        195766423245049 * f
        + 971821376 * f * f
        + 5127 * f * f * f
        + (1 << 47)
    ) // (1 << 48)


def v2_target(
    anchor: int, parent_height_delta: int, parent_ts_delta: int,
    *, emulate_early_clamp: bool = True,
) -> int:
    # Exact positive-range integer path used by the v2 case in difficulty.rs.
    ideal = parent_height_delta * 30
    exponent = trunc_div((parent_ts_delta - ideal) * 65536, 180)
    shifts = exponent >> 16
    frac = exponent - shifts * 65536
    net = shifts - 16
    # difficulty.rs returns GENESIS_TARGET unconditionally at net >= 46.
    # This can differ from the arithmetic shift when the anchor is tiny.
    if emulate_early_clamp and net >= 46:
        return FLOOR_TARGET
    if emulate_early_clamp and net <= -320:
        return 1
    raw = anchor * bch_factor(frac)
    raw = raw << net if net >= 0 else raw >> -net
    return max(1, min(FLOOR_TARGET, raw))


def work(target: int) -> int:
    # Saturation matches noid_chain's u256 chainwork representation.
    return min(U256 - 1, (U256 - 1) // target + 1)


def median(values: list[int]) -> int:
    return sorted(values[-11:])[len(values[-11:]) // 2]


def check_case(case: dict, source: dict) -> dict:
    height = case["base_epoch_height"]
    base_ts = (
        source["verified_mainnet_h210536_origin"]["timestamp"] + 120
        if height == 210540 else 1795000000
    )
    anchor_target = (
        int.from_bytes(bytes.fromhex(source["verified_mainnet_h210536_origin"]["difficulty_target_hex"]), "little")
        if height == 210540 else 1
    )
    offset = case["min_parent_future_offset_secs_to_floor"]
    assert v2_target(anchor_target, 0, 0) == anchor_target
    assert v2_target(anchor_target, 1, offset - 1) < FLOOR_TARGET
    assert v2_target(anchor_target, 1, offset) == FLOOR_TARGET

    prior = [base_ts - (10 - i) * 30 for i in range(11)]
    timestamps = []
    targets = []
    mtps = []
    for step in range(1, 25):
        if step <= 5:
            ts = base_ts + offset + (step - 1) * 30
        elif step == 6:
            ts = base_ts + 1
        else:
            ts = base_ts + 1 + (step - 6) * 30
        mtp = median(prior)
        assert ts > mtp, (height, step, "MTP")
        anchor_step = ((height + step - 1) // 6) * 6 - height
        assert anchor_step >= 0
        epoch_anchor_ts = base_ts if anchor_step == 0 else timestamps[anchor_step - 1]
        epoch_anchor_target = anchor_target if anchor_step == 0 else targets[anchor_step - 1]
        parent_ts_delta = prior[-1] - epoch_anchor_ts
        target = v2_target(epoch_anchor_target, step - 1 - anchor_step, parent_ts_delta)
        assert target == (anchor_target if step == 1 else FLOOR_TARGET), (height, step, "target")
        mtps.append(mtp)
        timestamps.append(ts)
        targets.append(target)
        prior.append(ts)

    assert mtps[5] == base_ts
    assert all(ts <= base_ts + 25 * 30 for ts in timestamps[5:])
    sixth_shifted_mtp = median(
        [base_ts - (10 - i) * 30 for i in range(11)]
        + timestamps[:5] + [base_ts + offset + 5 * 30]
    )
    assert sixth_shifted_mtp == base_ts + offset
    hard_work = work(anchor_target)
    floor_work = work(FLOOR_TARGET)
    assert floor_work == case["floor_block_work"] == 262144
    if height == 210540:
        assert hard_work == source["verified_mainnet_h210536_origin"]["block_work_u128"]
    claimed_work = min(U256 - 1, hard_work + 23 * floor_work)
    assert (claimed_work > hard_work) == case["beats_one_honest_hard_block_work"]
    assert (claimed_work > min(U256 - 1, 2 * hard_work)) == case["beats_two_honest_hard_blocks_work"]

    # All five shifted headers must be admissible before any descendant can be
    # admitted as a contiguous live chain. The fifth is the limiting header.
    earliest_complete_prefix_delta = max(timestamps[:5]) - base_ts - 120
    assert earliest_complete_prefix_delta == offset
    reported_clock_delta = 25 * 30
    rejected_shifted_at_reported_clock = sum(
        ts > base_ts + reported_clock_delta + 120 for ts in timestamps[:5]
    )
    assert rejected_shifted_at_reported_clock == 5
    assert timestamps[-1] <= base_ts + reported_clock_delta + 120

    return {
        "case": case["label"],
        "source_anchor_is_synthetic": True,
        "first_hard_header_mined_or_validated_by_source": False,
        "floor_headers_mined_by_source": case["floor_blocks_mined_with_real_pow"],
        "floor_offset_seconds_from_synthetic_anchor": offset,
        "earliest_clock_delta_for_all_shifted_headers_seconds": earliest_complete_prefix_delta,
        "shifted_headers_rejected_at_source_clock": rejected_shifted_at_reported_clock,
        "tip_passes_future_drift_at_source_clock": True,
        "one_hard_block_accounted_work": str(hard_work),
        "one_hard_block_expected_hashes_ceiling": str((U256 - 1) // anchor_target + 1),
        "one_floor_block_expected_hashes": floor_work,
        "source_assumed_branch_work": str(claimed_work),
        "work_from_23_mined_floor_headers_only": 23 * floor_work,
        "source_branch_work_is_from_unmined_first_block": True,
        "floor_blocks_needed_to_exceed_one_additional_equal_hard_block": (
            None if hard_work == U256 - 1 else hard_work // floor_work + 1
        ),
        "honest_blocks_at_30s_nominal_during_wait": offset / 30,
        "maximum_honest_blocks_before_anchor_exits_18_block_finality_window": FINALITY_DEPTH,
        "minimum_mean_honest_interval_to_keep_anchor_in_window_seconds": offset / FINALITY_DEPTH,
        "target_without_early_clamp_at_threshold": str(v2_target(
            anchor_target, 1, offset, emulate_early_clamp=False
        )),
    }


def main() -> None:
    source = json.loads(SOURCE.read_text())
    assert source["pinned_commit"] == "52938433a30b185ab007afaa15166a632cf2e82e"
    constants = source["constants"]
    assert (constants["epoch_length"], constants["median_time_blocks"],
            constants["max_future_drift_secs"], constants["v2_block_time_secs"]) == (6, 11, 120, 30)
    harness = (ROOT / "experiments/comp-sync-finality/src/bin/asert_mtp_conformance.rs").read_text()
    assert "let base_work = [0u8; 32]" in harness
    assert "let do_pow = mine_floor_pow && step >= 2" in harness
    assert "Seed 11 canonical ancestors" in harness
    cases = [check_case(case, source) for case in source["asert_mtp_anchor_reset_cases"]]
    output = {"source": str(SOURCE.relative_to(ROOT)), "cases": cases}
    OUT.write_text(json.dumps(output, indent=2) + "\n")
    print(json.dumps(output, indent=2))


if __name__ == "__main__":
    main()
