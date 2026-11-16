# NM-3C: redemption before launch graduation

Pinned source: `experiments/comp-system-hunter/pinned/` at `52938433a30b185ab007afaa15166a632cf2e82e`.

## Assessment

The reported pre-graduation “floor lock” is a real reachable state, but it is the expected effect of a holder voluntarily burning tokens for their proportional backing. The burn does not draw from the curve or pool. It reduces the separate backing reserve and its outstanding supply. The curve keeps its cash and inventory; the burned balance no longer exists, so no holder can sell those units back to the curve. The resulting minimum sold quantity equals the burned units only if all other externally held units are sold. This does not establish theft, a halt, or a free griefing action. The redeemer has already paid the curve price and gives up the tokens; the network fee adds to that cost.

This separation is explicit in `docs/protocol/native-finance.md:23-43,65-82`. In code, launch creation puts all initial supply into the reserve obligation and the unsold portion into launch inventory (`noid_tx/src/native_assets/launch/owner.rs:87-134`, `launch.rs:175-190`). Redemption accepts only a reserve and holder-owned token balances, then checks the exact reserve successor (`noid_tx/src/native_assets/transition.rs:160-175,237-252,393-407`). The wallet quotes this as `BurnToken` and includes the backing payout in the funding calculation (`noid_rpc/src/server/native/planner.rs:465-502`).

The isolated control in `src/main.rs` checks a 100,000-token launch with a creator purchase of 10,000 tokens, then a burn of all 10,000. It checks that the owner transition accepts both actions, another owner cannot redeem the balance, launch inventory cannot be used as a redeem input, and buying the remaining 50,000 sale tokens quotes graduation with exact conservation. These are transaction-relation controls, not a mined-block or proof-verifier test.

## Gemini boundary subclaims

The original seam harness uses `launch_supply_from_whole(1_000)` in its first launch test and again in the trade check (`native-market-seam/src/main.rs:85,609`). The pinned minimum is 100,000 whole tokens (`noid_tx/src/native_assets/launch.rs:22-44`), so that harness cannot have completed those paths as written.

The multi-batch `total > MAX_PAGED_SPEND_INPUTS` guard in `noid_chain/src/block/payload/trades.rs:229-234` is a defensive bound, not an observed failure. `MAX_PAGED_SPEND_INPUTS` and the global block live-input cap are both 1,020 (`noid_tx/src/paged_spend.rs:43`, `noid_chain/src/consensus/params.rs:158`). The scheduled stream checks aggregate live inputs before trade payload processing (`noid_chain/src/block/native_assets.rs:337-386`). No separate dynamic multi-batch edge case was demonstrated here.

At a `log_slots` expansion, trade placement uses child geometry (`noid_miner/src/template/trades.rs:150-166`), while quoted fees use parent State (`noid_miner/src/template/trades.rs:299,443`; `noid_chain/src/state/native_assets/trade.rs:173-181`). The admitting preparation accepts either parent or one-step child geometry and checks output indices against child geometry, while calculating required fees from parent State (`noid_chain/src/state/native_assets/trade.rs:210-251`). This source review supports consistency for this narrow boundary; it is not a runtime consensus test.

## Run

Executed locally with exit code 0:

```sh
CARGO_BUILD_JOBS=2 cargo run --offline --manifest-path experiments/verification-redeem/Cargo.toml
```

```text
launch creation transition accepted: supply=100000000000 purchased=10000000000 curve_cash=38888889
redemption transition accepted: burned=10000000000 payout=10000000 reserve_after=90000000 outstanding_after=90000000000
purchase cost minus redemption payout, before fees and launch backing deposit: 28888889 micronoid
negative controls: foreign_owner=Authority launch_as_balance=Shape
graduation quote: remaining_sale=50000000000 additional_cash=361111111 pool_cash=400000000 pool_tokens=40000000000
```
