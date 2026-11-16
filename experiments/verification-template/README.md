# Bounded miner-template conformance fixture

Target source: `experiments/comp-system-hunter/pinned/`, copied from Parano1d commit `52938433a30b185ab007afaa15166a632cf2e82e`.

Run: `CARGO_BUILD_JOBS=2 cargo test --offline --manifest-path experiments/verification-template/Cargo.toml --lib -- --nocapture`.

The test calls the production `build_node_owned_block_template` implementation on two states with the same exact root, live count, and selected transaction. One state has a second populated segment resident; the other has that segment evicted. The selected transaction reserves the only two holes in the first segment. It also checks the empty-reservations predicate used by `TemplateChainSnapshot::hydrate_coinbase_reuse_segment`: the precheck sees a hole, although the selected payment reserves both holes. The passing run demonstrates `Err(NoCoinbaseSlot)` in the evicted state and successful construction in the hydrated control.

This is a **component result**. It does not show that a valid persisted chain and the actual `TemplateBuilder` metadata snapshot reach that state, nor that a release daemon or network halts. `TemplateChainSnapshot::from_context` requires a durable `MdbxChainContext` with matching state/header roots and segment records; manufacturing those with raw MDBX writes would not establish a valid-chain outcome. A mainnet-depth fixture needs all 256 segments populated and one 65,536-slot segment nearly full, which requires a much larger authenticated block sequence.

The mainnet `log_slots` is 24, but this fixture uses the same production allocator at depth 17 to keep the setup bounded. The economic occupancy threshold for Gemini's separate stale-fee scenario remains untested here.
