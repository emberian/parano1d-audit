# CS-1 conditional seam: observed result

Pinned source: `52938433a30b185ab007afaa15166a632cf2e82e`. The bounded release run exited 0; see `run.log` and `src/main.rs`.

## What ran

- Twenty-four local blocks were accepted through PoW, native consensus, and MDBX. Their HistoryStep terminal verifier was stubbed.
- A cryptographically authorized 3-input/4-output `CreateLaunch` was admitted at a **synthetic** 50% occupancy view with exact fee **13,100 µNOID**. That view used the real low-occupancy UTXO State but a fabricated active-slot count and matching fabricated parent header/hash. It is not a valid chain state.
- After the real height-24 low-occupancy block, `AsyncMempool::on_new_block` retained the overpaying intent. `select_for_v2_mining` returned it for the next candidate, and `checked_financial_owner` still passed.
- The production chain template function `build_node_owned_block_template_with_native_data` rejected the selected candidate with `StateApplyError("financial block: Fee")`; the authentic parent’s exact fee was **10,600 µNOID**. An empty template at the same parent succeeded. A newly signed 10,600-µNOID `CreateLaunch` also admitted, selected, and built successfully.

The relevant source path checks exact financial fees at admission (`noid_mempool/src/native.rs:162`), but reorg/new-block eviction tests only `fee < current_minimum` plus stateless native context (`noid_mempool/src/pool.rs:789-799`). The chain template propagates `native::checked_candidate` errors (`noid_chain/src/consensus/template.rs:787`), and `noid_miner::TemplateBuilder` maps an inner builder error to `None` (`noid_miner/src/template.rs:550-572`).

## Limits

This is a conditional component reproduction. A valid mainnet-geometry pressure crossing would require at least **8,388,608 live slots** before dropping below 50%; none was built. The fixture did not call `noid_miner::TemplateBuilder`, prove a HistoryStep terminal, start a release daemon, or demonstrate a chainwide mining halt. The evidence supports a real mismatch in the source paths and a reproduced inner-template failure under a synthetic pressure precondition. The stronger claim that an actual local network was halted remains unproven.

Gemini’s earlier CS-1 description also quoted obsolete fee constants. The pinned code charges a 5,000-µNOID base, 100 per input, 700 per output, and 2,500 per net-new slot before the occupancy multiplier (`noid_chain/src/consensus/params.rs:428-446`).
