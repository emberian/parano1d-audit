# Constraint-systems audit: Pruv compilation, the integer-program circuit, and fold/collision composition

Source: `/Users/ember/dev/parano1d-break/parano1d` at `52938433a30b185ab007afaa15166a632cf2e82e`.
All created files are confined to this directory. No live node was started; every check runs the real builder (`noid_ivc_core::field_circuit::FieldR1csBuilder`), the real production constraint code (`noid_recursive::acceptance::trace::integer_program::{bind_program, bind_program_v2_1}`), the real compiler (`noid_pruv::compile`), and the real Z3 checker path (`noid_pruv::verify::verify`).

## What the constraint system actually is

Pruv does not compile to a custom arithmetic circuit per contract. `noid_pruv::compile`
(`noid_pruv/src/compile.rs:688-779`) lowers a typed source program to a **fixed 16-instruction
integer program** over 4 registers (`noid_tx/src/experimental_object/integer_program.rs`, ABI 3),
embedded as committed fields of a v2 object opening. The chain's constraint system for these
contracts is a **fixed-shape circuit twin** of that interpreter
(`noid_recursive/src/acceptance/trace/integer_program.rs:245-320`) inside the HistoryStep block
relation. Two facts raise the stakes:

1. **Consensus never executes the integer program natively.** `integer_program::execute` has no
   caller in `noid_chain`/`noid_block` (grep across the tree: only `noid_recursive` circuit,
   `noid_tx` definitions, wallet preview, RPC/GUI, benches). The circuit is the sole consensus
   executor of Pruv state transitions — circuit soundness is the whole game.
2. Contract witnesses are **never serialized in the block body**
   (`noid_recursive/src/acceptance/history_step.rs:176-178`); they exist only as proof-internal
   wires bound to the body through the object-root commitments below.

### Construct-to-constraint mapping (verified line by line)

| Pruv/VM construct | Native semantics | Constraint pattern | Binding |
|---|---|---|---|
| Instruction word | 20-bit descriptor + u64 immediate, `from_fields` rejects reserved encodings (`integer_program.rs:225-241`) | `range_check_bits` 20/64 bits, one-hot `selectors` with invalid-index sum pinned zero (`trace/integer_program.rs:102-122,264-271`) | encoding validity is enforced in-circuit, not assumed |
| Opcode mux | 10 opcodes | one-hot product selectors; Σ invalid = 0 with boolean bits ⇒ exactly one valid selector = 1 (telescoping identity Σ all = 1) | `trace/integer_program.rs:267` |
| Operand mux | 24 operands incl. context | one-hot select over 24 lanes (`:269-278`) | context lanes pinned one level up (below) |
| Predicate | raw value must be 0/1 **before** inversion; skip iff (raw==1)==inverted (`integer_program.rs:371-377`) | `boolean(predicate)` unconditionally, then `enabled = live·(raw XOR inverted)` in char 2 (`trace/integer_program.rs:272-275`) | matches native error-before-skip |
| Add/Subtract | checked, overflow/underflow error (`integer_program.rs:385-390`) | ripple-carry full adder, `bad_carry = enabled·(op2+op3)·(carry+op3)` pinned zero (`:148-175,288-290`; compact variant `:178-221`) | fail-closed carry, both arithmetic variants |
| LessThan/Equal | unsigned compare | MSB-First borrow fold / XNOR product chain; v2.1 uses an inverse-gadget is-zero (unique given the two pins) (`:160-166,188-201`) | deterministic given range-checked bits |
| AssertEqual/AssertLessOrEqual | error, destination kept | `enabled·op8·(left+right) = 0`, `enabled·op9·(¬less∧¬equal) = 0`; destination mux = keep (`:291-307`) | skip-when-inactive matches native |
| `when` guard (Pruv) | compiled: reset scratch to 0 unconditionally, compute guard under parent predicate, body under scratch predicate (`compile.rs:549-569`) | same instructions in-circuit; booleanity via LessThan/Equal outputs | inactive parent leaves guard 0 (P9 probe) |
| Final state | `pack_state(registers)` | `live·(packed_registers + next) = 0` (`:315-319`) | next is forced to the execution result when live |

**Substrate soundness primitives** (why the per-wire determinism argument closes):
rows are appended in wire order with `C = I` (`field_circuit.rs:911`, `field_r1cs.rs:1826-1835`),
so row *i* defines wire *i*. `alloc_bool` emits `w·w = w` (`field_circuit.rs:1112-1116`), `mul`
emits `x·y = w` (`:1119-1152`), `pin_f128` emits `expr + expected = 0` via a slack wire
(`:1210-1229`), and `alloc_f128` rows are tautologies that `bind_linear_pin_in_free_row`
(`:698-757`) rewrites into pins without dropping anything. The only intrinsically two-valued rows
are booleanity rows, and every one of them sits inside a gadget whose sibling pins force the
value uniquely: the range decomposition `Σ bᵢ·φ(2ⁱ) = expr` is injective (tower basis linear
independence, `trace/mod.rs:420-434`), and the selector family telescopes to 1.

## Findings

### F1 (headline, negative): witness uniqueness holds — no second satisfying witness found on any probed constraint set

**Severity: none (blocker statement below).** The question this lane was asked — do two distinct
executions/witnesses ever satisfy one constraint set with different observable outcomes —
answered **no** on everything probed:

- **8870 build-and-check configurations** (10 builder configurations × both arithmetic variants,
  over 120 random programs at u64 boundaries plus 9 directed cases) with **0 failures**: every
  honest native execution satisfies; every forgery candidate is rejected by
  `matrix.satisfies()`. Forgery candidates included wrap-add/wrap-sub executions, non-boolean
  predicates interpreted both as true and as false, writes from inactive instructions, skips of
  active instructions, stale-next under tampered context, all 128 single-bit flips of a
  non-trivial program's next state, and every reserved instruction encoding (opcodes 10-15,
  operands 24-31, predicate sources 6-7, descriptors ≥ 2²⁰, immediates ≥ 2⁶⁴).
- **Blocker (structural, not incidental):** given the committed inputs (program, current,
  context, live), every wire of the circuit is *uniquely determined* — each non-booleanity wire
  by its defining row over earlier wires, each booleanity wire by its gadget's aggregate pins
  (see mapping above). A second witness with the same committed inputs cannot exist; a second
  *outcome* would require breaking one of: char-2 gadget identities (majority carry, XNOR
  equality — verified exhaustively over boolean inputs by the boundary battery), the φ-map's
  tower-basis injectivity, or the pin rows themselves.

### F2 (Low, process): the production v2.1 compact-arithmetic constraint path had zero direct tests

`bind_program_v2_1` / `integer_results_v2_1` (`trace/integer_program.rs:178-221,234-243`) is
selected for `ScheduledV2` profiles with authorization activation
(`block_slots.rs:2974-2981`), and is the variant with the more delicate gadgets (inverse-gadget
equality, compact carry `(adjusted+a)(carry+a)+a`). Every in-tree test exercises the v1 path
(`trace/integer_program.rs:355` test helper; full-relation tests pass `false` at
`block_slots.rs:4112`). Not a bug — the algebra is correct and now covered — but until this lane,
the *production-selected* arithmetic for v2.1 contracts had never been differentially tested
against native execution. This battery covers it under all 10 builder configurations
(`results-ip.txt`).

### F3 (negative): public-input binding holds end to end

The full chain of custody for a Pruv execution, each link pinned in-circuit:

1. `program`/`current`/`next`/policy cells → **object-root sponge wraps**
   (`SPINE_CONTRACT_{CODE,POLICY,OBJECT}_DOMAIN` full permutations,
   `noid_ivc_core/src/deep_chain/spine.rs:296-391`): old root commits policy+**current**,
   new root commits policy+**next**, code digest commits all 32 program words.
2. Object roots pinned to committed body leaves: old root ↔ `TX8X2_LEAF_INPUT_OWNER`, new root
   ↔ `TX8X2_LEAF_OUTPUT0_OWNER` gated on live ∧ ¬terminal (`block_slots.rs:1516-1529`);
   terminal recipient pinned to claim/refund recipient by deadline (`:1044-1067`).
3. Context values pinned to spine leaves (`block_slots.rs:997-1001`); `before_deadline` is
   *computed in-circuit* as strict `lt(height_bits, deadline_bits)` (`:929-939`), with height
   from the authenticated block and deadline from the committed opening.
4. Body leaves → TX8X2 body hash (spine replay) → TAG_TXROOT count-bound Merkle wrap pinned to
   the header root (`block_slots.rs:1146-1160,2880-2889`; `noid_chain/src/tx_tree.rs:33-38`) →
   header → accumulator/PoW.
5. The `live`/`terminal`/`has_payout` gates are range-checked bits of the authenticated
   validity-bitmap leaf (`trace/action_surface.rs:192-205`) — never free witness.

**Documented design behavior (not a hole):** the *component* circuit takes context wires as
advice — a consistent (tampered height, matching next) pair satisfies the component in all 10
configurations (`results-ip.txt`, "context advice" line). This is the two-level design: the
authentication is the level-up pins in item 3, which the in-tree full-relation tests exercise
(`object_execution_tests`, all green — see controls) and which my P3/P8/P9 probes confirm
semantically (a decrement that did not happen at the authenticated height is rejected).

### F4 (negative, with one precise composition answer): the known compressor collision does not compose into any semantic binding

The 2¹⁶-cost collision lives in `compress_flat_feed_forward_with_tag` under `TAG_CAPSNODE`
(reproduced here from the pinned pair, digest `b2cf54a4…`). The direct Rust consumers are the
PCS commitment builders: the IVC PCS Merkle (`IVCPCSN_`, `noid_ivc_core/src/merkle.rs:128`),
`noid_fri_binius/src/capsule.rs:193` (capsule tree), and the in-circuit PCS-Merkle replay
(`trace/self_verify.rs:4399-4404`). In `ff_merkle.rs` the only `compress_flat_feed_forward_with_tag`
calls are in `mod tests` (`:363-419`); the production code (`:90-360`) is the *in-circuit
verification family* for those same trees — its path-start binding pins the entry leaf, its
CR-chain zero-check carries each node's recomputed digest, and "the caller can then pin the
**transcript-observed root** directly to one committed cell" (`ff_merkle.rs:30-34`).

So, precisely: **yes**, the fold/PCS layer does consume attacker-influenced digests through the
colliding domain family — the prover of a block chooses the capsule/PCS commitments, and the
child proof's replay verifies openings against them (that exposure is the basefold-chains
lane's finding, and whether a node-pair collision can be exploited depends on FS query
unpredictability, not on anything in this lane). **No** semantic binding does: no wire of the
Pruv/object path — program, state, context, amounts, recipients, owners — is ever hashed under
`compress_flat_feed_forward_with_tag` or bound by a tree built with it. The semantic chains all
use **full-sponge** constructions: `compress_with_tag` for state/tx-root/segment/claim nodes
and one-permutation duplex wraps for the object roots. Empirically (`results.txt`, section 2):
the colliding pair does **not** collide under `TAG_EXSTNOD`, `TAG_TXROOT`, `TAG_SEGMENTTREE`,
or `TAG_CLAIMS`; and the object-root wraps are **single-bit sensitive in every committed
cell** — 32/32 program words move the code digest and both roots; `current` moves *only* the
old root; `next` moves *only* the new root. A "two payloads, one binding" forgery of contract
semantics needs a collision *in these wraps*, which is a full-sponge (2-permutation)
cryptanalytic problem, not the known feed-forward collision — the two never meet.

### F5 (negative): no in-field wrap or negative-value path exists

All numeric lanes are range-checked (u64 amounts/heights, u32 slots, 20-bit descriptors,
64-bit immediates) and all arithmetic is carry-checked integer arithmetic — in **both** the
native reference (`checked_add`/`checked_sub`, `integer_program.rs:385-390`) and both circuit
variants. The classic `p−1` "negative balance" attack is structurally unavailable: semantic
values are never reduced modulo the field. The hand-written wrap cases the contract-rights
generator would never emit — `remaining(1000) -= payout(1005)`, counter at `u64::MAX` + 1,
overflow inside a `require`'s intermediate — all fail closed in native, in every circuit
configuration, and in the Z3 postcondition checker (`results-puv.txt` P1/P2/P5).

### Z3 postcondition checker (verify.rs) — encoding faithful on adversarial shapes

`noid_pruv/src/verify.rs` emits an SMT-LIB2 twin of the interpreter. Audited points: per-
instruction predicate booleanity is asserted (`:204-208`) before the active-branch gate
(`:209-212`); assert opcodes keep the destination (`:232-235`); Add/Sub overflow guards are
asserted under the active branch (`:220-227`); immediate width comes from the BV encoding; the
three verdict classes were each demonstrated through the real `verify()` path (P6
Counterexample, P7 Infeasible, P1-P5/P8/P9 Verified). One documented conservative behavior: the
initial state is universally quantified (`:157,175,187-192`) — the checker proves postconditions
for *all* initial states, which is sound for the deployed contract (whose initial state is
fixed at compile time) but can report a Counterexample for states the contract never has — a
false-alarm direction only.

## Negative and control results

- 120 seeded random programs (valid encodings, boundary-biased immediates and context): 37
  executed successfully, 83 failed natively; **circuit verdict matched native verdict in all
  8870 configurations**; 694 random forgery candidates + all directed candidates rejected.
- Matrix shape is value-independent: `structural_statement_digest` identical across every
  instance per (variant, builder-configuration) — no data-dependent constraint laundering.
- Dead slots (live = 0) accept arbitrary `next` **by design** (ghost rows keep one
  content-independent matrix); safe because `live` is an authenticated bitmap bit and every
  object-root pin is gated on it.
- In-tree controls at the pinned commit (this machine, lane-local target dir):
  `acceptance::trace::integer_program` — 5/5 ok (172 s);
  `acceptance::block_slots::object_execution_tests` — 4/4 ok, including
  `integer_instructions_and_assertion_failures_are_constrained`,
  `every_integer_body_operand_matches_native_and_height_cannot_be_substituted`,
  `spending_rules_match_native_checks_without_witness_dependent_rows`,
  `scheduled_templates_share_one_relation_and_enforce_claims_and_recovery`.
- HistoryStep matrix bank: class matrices are digest-authenticated leases
  (`history_step_bank.rs:49-70`); the fold consumes the parent proof's matrix claim and the
  terminal evaluates claims against locally authenticated matrix rows — no prover-chosen digest
  enters a semantic binding.
- No accepted semantic forgery, no ghost-write, no wrap, no context substitution, and no
  collision-composited double-payload was demonstrated anywhere.

## PoC implementation and reproduction

Three binaries in this directory, all depending on the pinned checkout by path (read-only;
`CARGO_TARGET_DIR` is lane-local):

```bash
cd /Users/ember/dev/parano1d-break/experiments/constraint-systems
CARGO_TARGET_DIR="$PWD/target" cargo run --bin ip_battery        | tee results-ip.txt
CARGO_TARGET_DIR="$PWD/target" cargo run --bin pruv_adversarial  | tee results-puv.txt
CARGO_TARGET_DIR="$PWD/target" cargo run --bin domain_separation | tee results-dom.txt
```

- `ip_battery.rs` — the witness-uniqueness battery (10 builder configurations × 2 arithmetic
  variants; random differential + wrap/predicate/encoding/flip lie battery). Requires nothing
  external.
- `pruv_adversarial.rs` — 9 hand-written adversarial Pruv programs (P1-P9) through compile →
  native → circuit → Z3. Requires `z3` at `/opt/homebrew/bin/z3` (override the `Z3` const).
- `domain_separation.rs` — collision reproduction, cross-domain non-transfer, and object-root
  single-bit sensitivity.

Final lines (see `results-ip.txt`, `results-puv.txt`, `results.txt` for full tee'd output):

```text
TOTAL: 8870 build-and-check configurations, 0 failures
VERDICT: witness uniqueness held on every probed constraint set; no alternate satisfying witness found
Pruv adversarial: 0 failures
domain separation: 0 failures
```

In-tree controls (both re-run at the pinned commit, lane-local target dir):

```bash
CARGO_TARGET_DIR="$PWD/target" cargo test --manifest-path ../../parano1d/Cargo.toml \
  -p noid_recursive --lib 'acceptance::trace::integer_program'          # 5 passed (172 s)
CARGO_TARGET_DIR="$PWD/target" cargo test --manifest-path ../../parano1d/Cargo.toml \
  -p noid_recursive --lib -- 'acceptance::block_slots::object_execution_tests'  # 4 passed (276 s)
```

## Assurance limits

- The uniqueness argument is structural (wire-order row definitions + gadget algebra) plus a
  directed lie battery; it is not an exhaustive search over the witness space of the full
  HistoryStep relation. The battery runs the production component circuits, not the end-to-end
  recursive prove/verify cycle for a Pruv-carrying block (prover-scale run was out of lane
  scope; the binding path is covered at R1CS level by the in-tree full-relation tests above).
- Builder optimizations are tested as {none, each single flag, all three} per arithmetic
  variant; runtime parts can wire-encode any subset (`runtime_parts_codec/capacity.rs:6-31`),
  and every subset class is either a tested configuration or a subset of the all-three
  interaction run.
- The collision-confinement result is demonstrated for the known colliding pair and the complete
  call graph of the colliding function. A *new* collision in the full-sponge `compress_with_tag`
  or the object-root wraps would be a different cryptanalytic result (2-permutation structure)
  and is not probed here — that is `noid_soundness` territory.
- Z3 (homebrew build at `/opt/homebrew/bin/z3`) is trusted for the postcondition-checker
  probes, as the production CLI does; the checker's resource limits (10 s query / 30 s wall,
  `verify.rs:267-271`) were never hit by these programs.
- No upstream source was edited; the pinned checkout was used read-only via path dependencies.
