# parano1d-break provenance

Adversarial audit workspace for the "parano1d" L1 (noid_* crates) by @ignotus_nemo.
Purpose: technically rigorous, honestly-scoped findings for a public rebuttal.
Discipline: only claim what a PoC actually demonstrates; negative results are results.

- Target checkout: `parano1d/` at pinned commit `52938433a30b185ab007afaa15166a632cf2e82e`
  (its worktree may carry unrelated pre-existing modifications — treat as read-only reference).
- Original session: kimi K3.1 Pro, `session_506c8a03-2089-4e2c-a141-629c1e2bb8a0`,
  transcripts under `~/.kimi-code/sessions/wd_parano1d-break_1aa8d980e0aa/`.
  It ran 29 sub-agents; 20 lanes completed, 10 died at 02:43–02:55 on the weekly quota
  (external-miner finished on disk before its report reached the root).
- `experiments/kimi-agent-reports/` — final message of each kimi agent, extracted from the wire.
- `experiments/kimi-root-narrative.md` — the root's running-findings summaries, in order.
- `poc/` — TAG_CAPSNODE production collision reproducer (72,274 evals / 0.128 s);
  `poc/src/bin/verify_collision.rs` is hardcoded-inputs-only verification.
- Lane status at snapshot: completed = address-preimage, basefold-chains, block-validation,
  build-artifacts (report in kimi-agent-reports/agent-25.md; root verification was interrupted),
  capsule-chains, contract-rights, external-miner, fork-replay, identity-encoding,
  ledger-fairness, native-intents, recursive-sync, state-consensus, transcript-chains,
  typed-binding, uc-composition, version-domains.
  Dead mid-lane (code on disk unverified) = rpc-api, p2p-resources, storage-reorg,
  wallet-safety, prover-unsafe, dex-economics, nft-marketplace, token-accounting.
- `experiments/*/pinned/`, `pinned-src/`, `source/` are regenerable copies of the target at the
  pinned commit (gitignored).

Taken over 2026-11-15 by Claude Code (glm-5.3) at ember's request.
