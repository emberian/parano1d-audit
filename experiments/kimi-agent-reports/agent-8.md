# agent-8: Attack native intent execution

(A final message from the kimi session wire; status may be completed OR died mid-lane)

## Result

The claimed builder-controlled `output = minimum_output` / `protocol_fee = fee_limit` vulnerability is **not present at the accepted-execution boundary**. I could construct those values with low-level witness APIs, but both block/template preparation and the recursive quote relation rejected their use as forged executions.

**Severity: no vulnerability confirmed as claimed.** No live network or transactions were used.

The capsule authorizes a limit order—asset, side, input, minimum output, fee ceiling, recipient, funding incarnations, chain/epoch, and expiry—not an exact clearing price. The actual price is instead determined by the authenticated parent market state and canonical sequencing.

## Concrete evidence

- `noid_tx/src/native_assets/trade/sequencing.rs:113-175` derives fills with `engine.apply(policy.side, policy.gross_input, policy.minimum_output)`. There is no builder-supplied output parameter.
- `noid_tx/src/native_assets/trade/execution.rs:123-159` regenerates canonical claims in `from_claims` and requires exact input/output equality.
- `noid_chain/src/state/native_assets/trade.rs:144-155` recomputes parent-State fee quotes before calling `TradeExecution::from_claims`.
- `noid_chain/src/block/native_assets.rs:718-748` invokes that checked path during block materialization and verifies fee, batch commitment, and auxiliary openings.
- `noid_recursive/.../sequential_trade/kernel.rs:345-384` routes each constrained `step.quote.output` into `bind_trade_payouts` and then binds payouts to actual block actions.
- `noid_recursive/.../sequential_trade/payouts.rs:170-177` independently checks `spent_input <= gross_input` and `output >= minimum_output`.
- `noid_recursive/.../sequential_trade/fees.rs:42-97` derives fee quotes from constrained parent fee context rather than accepting a builder quote.

The experiment produced:

```text
canonical host fill: spent_input=10000 output=9871 minimum=1 network_fee=9800 fee_limit=100000
canonical NativeIntent accepted by block/template preparation: yes
minimum-output substitution rejected: Wire(Trade(Outputs))
low-level fee_limit execution constructed, block/template preparation rejected it: Wire(Trade(Outputs))
isolated quote_charge accepts fabricated outcome=minimum and fee=fee_limit (2 payouts);
this witness is not an accepted execution
parent-State fee quotes and exact claims independently reconstructed: yes
checked canonical transition applied to local State: yes
recursive market trace: canonical output=9871 satisfies R1CS;
replacing output wire with minimum=1 fails R1CS
```

Thus, `quote_charge` accepting a fabricated `TradeOutcome` is only witness construction, not an authorization or acceptance bypass. A successful attack would require bypassing both exact host reconstruction and the recursive relation, or forging the enclosing proof—not merely controlling block building.

## Same-anchor replay and context checks

`parent_id` is deliberately outside `authorization_id`; it affects funding priority. A real V4 owner capsule verified at adjacent heights under the same epoch anchor, while wrong chain/anchor checks failed.

After applying the canonical trade, replay at the next height with the same anchor and a different parent failed:

```text
same-anchor, next-height, different-parent replay rejected: Inputs
same capsule verifies at adjacent heights under one epoch anchor: yes
same capsule with wrong epoch anchor or chain rejected: yes
parent_id is outside the capsule and changes no single-policy fill: output=9871
```

The replay stop is `noid_chain/src/state/native_assets/trade.rs:297-329`: every input must still equal the exact parent-State `(amount, creation_id, owner_root)` incarnation. The executed pool and funding slots were empty. Reusing their slot indices later with new creation IDs would not satisfy the old capsule.

Therefore:

- The same capsule can remain cryptographically valid during its signed validity window.
- It cannot double-spend consumed sources on one State.
- Execution on competing forks sharing the same unspent parent incarnation remains possible, consistent with ordinary UTXO fork semantics.
- Exact preconditions for a same-State replay would be restoration of the original pool and funding slot incarnations, which monotone creation IDs prevent.

## Files and verification

Created only in `experiments/native-intents`:

- `Cargo.toml`
- `Cargo.lock`
- `src/main.rs` — canonical execution, forged output/fee, real owner-capsule, replay, and direct R1CS experiments.
- `target/` — local build artifacts.

Commands from that directory:

```bash
CARGO_TARGET_DIR=$PWD/target cargo check
cargo fmt --check
CARGO_TARGET_DIR=$PWD/target cargo run --release
```

All final commands passed. The release run completed in about two seconds after compilation; capsule generation took approximately 0.64 seconds. Upstream emitted pre-existing dead-code/unused warnings only.

The experiment exercises the production block/template preparation and State application functions plus the production recursive market gadget. It does **not** generate a complete proof-carrying `HistoryStep` block, because no exploitable execution survived the earlier checks. Repository HEAD remained `52938433a30b185ab007afaa15166a632cf2e82e`; I made no edits under `parano1d`.
