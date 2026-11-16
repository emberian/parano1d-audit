# CD-3: financial definition capacity

This is a bounded local reproduction against Parano1d source revision
`52938433a30b185ab007afaa15166a632cf2e82e`. The copied source under
`../comp-system-hunter/pinned/` matches that revision for the relevant V2
configuration and candidate-preparation files. No network peer is contacted.

## Reproduce

```sh
CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=experiments/comp-consensus-diff/target \
  cargo run --offline --manifest-path experiments/verification-definition-capacity/Cargo.toml
```

The first run generates three genuine V4 owner authorization proofs and caches
their public, serialized intents under ignored `auth-cache/`. Later runs load
and reverify those proofs against the fixture parent. The saved successful
output is `run.log`.

## Observed result

The fixture uses a supported **research candidate** shape: 63 pages, 256
inputs, four native headers, 64 action records, one market and therefore five
auxiliary definitions. It constructs a synthetic parent state at height 144,
then two custom child blocks from the same parent. Each `CreateLaunch` buys
the entire sale inventory at creation. Its resulting market pool refers to
both a launch definition and a market definition. All creators own their
funding inputs and carry freshly generated, verified V4 proofs.

| Groups in custom block | Definitions | Native checks, payload, state | Candidate preparation with real proofs | Definition binder |
| ---: | ---: | --- | --- | --- |
| 2 | 4 of 5 | accepted | accepted | succeeds |
| 3 | 6 of 5 | accepted | accepted | panics at `financial_definitions.rs:276` |

The production mempool budget selected two of the three signed intents under
this same five-definition limit. The custom three-group block bypassed that
selection path. The panic was caught by the fixture so it could finish and
record all outcomes.

The code path from candidate input to the panic is visible in source:
`noid_block/src/candidate_history.rs` checks headers and action records but
does not count auxiliary definitions. `noid_recursive/src/acceptance/block_slots.rs`
filters `components.native_openings` to auxiliary definitions and passes the
list into `bind_financial_sequential_preparation`, which calls
`bind_financial_definitions` and its asserted table binder. The observed run
called the table binder directly with exactly the same sorted definition list.

## Mainnet-bank extrapolation

The pinned `v21-runtime-metadata.bin` decodes to a Native class with 63 pages,
504 live inputs, five native headers, 128 action records, four trade markets,
64 trade-funding inputs, 34 settlement actions, and **nine** auxiliary
definitions. The artifact decode is checked in
`../verification-opcodes/src/lib.rs`; its corrected pinned-metadata test passed
and authenticated the bank digest (`../verification-opcodes/capacity-output.log`).

Five independent immediately graduating launches would require five headers,
five live inputs, 25 live outputs, and ten distinct auxiliary definitions.
They appear to fit the other relevant capacities while exceeding nine
definitions. They are CreateLaunch admissions, not settlement or trade-batch
operations. This **five-launch, ten-definition path is a source-based
extrapolation**, separate from the observed three-launch, five-capacity
research-class result above. The five-launch block was not run against the
pinned mainnet bank.

## Boundary of the result

The parent header and funded state are synthetic. The run verifies native
block checks, canonical payload construction, state materialization, real
owner authorizations, candidate preparation, and the definition binder. It
does **not** build or submit a full V2.1 HistoryStep proof, accept a block on a
persisted devnet, or show a network-wide halt. A full prover invocation might
reject another boundary before reaching this binder; the source call path
supports the inference that a prepared over-capacity candidate would reach
it, but that invocation remains unobserved. Ordinary miner selection budgets
definitions and avoided the over-capacity candidate in this run. No peer path
that makes an honest node construct such a candidate was demonstrated; an
unproved custom block cannot establish an accepted-chain halt.
