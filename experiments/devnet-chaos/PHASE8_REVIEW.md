# Phase 8: the release victim rejected the poison fork

Target source: `52938433a30b185ab007afaa15166a632cf2e82e`. The run in
`results.txt` began on 15 November 2026 and used loopback nodes. It did not
contact the public network. This review separates the recorded outcome from
the runner's intended attack narrative.

## Observed result

The runner froze an honest branch at height 31, then mined a separate branch
to height 61 using a clock offset of ten years. A fresh victim was seeded to
the honest relay under an anonymous serving stall and to the poison node.
Both peers offered snapshot manifests. The poison offer won the five-second
candidate comparison by advertised bridge work, but the victim did **not**
install it. Its log records:

- `header inventory failed native validation ... h=1: BadTimestamp`;
- `snapshot candidate rejected ... HistoryStep target tip exceeds local future drift: BadTimestamp`;
- acceptance and installation of the honest height-12 snapshot, followed by
  an authenticated exact suffix to the honest height-31 tip.

The victim's recorded final tip is the honest `d6fc8017364927c8`, not the
poison `f5421d87e3f3e5a5`. A second fresh joiner seeded only to the victim
also finished at honest height 31. The runner printed
`victim_on_poison=False reserved_to_next=False`. Its following static `SCOPE`
sentence says that poisoning was demonstrated; that sentence is contradicted
by the result immediately above it. `PHASE8 OK` means the script reached its
end, not that the attack worked.

## What this controls, and what it does not

The ten-year clock offset affected the poison *tip*. The release verifier's
tip future-drift check therefore rejected the genuine HistoryStep terminal.
This is an effective defense against this fixture. The earlier C26 component
test placed the future timestamp in a *middle* header and used a present-day
tip, with a stubbed recursive terminal. Phase 8 does not run that case with
the release verifier. A valid terminal for that shape, competing work, and
live installation remain open.

The result also does not show that the anonymous serving stall deprived the
victim of honest headers. The victim installed and followed the honest
branch. The earlier phase-7 log independently shows installed height zero
coexisting with a selected honest HeaderDAG tip at height 150; see
`header-progress-review.txt`. A snapshot candidate's higher advertised work
is not authority by itself: staged headers, a genuine terminal, the tip-time
check, and selected-work comparison are distinct gates.

## Reproduction and evidence

The script is `scripts/phase8_live_c26.py`. The saved run is the `PHASE 8`
section of `results.txt`. The checked, tracked excerpt is
`phase8-verdict.txt`, produced by `scripts/check_phase8_result.py` from the
local `logs/p8-victim.log` and `logs/p8-victim2.log`. Those full debug logs
are local run artifacts and are not committed. The script created and later
stopped its own loopback nodes. The exact branch hashes above are prefixes
printed by `getChainInfo`. Do not reuse its all-future poison chain as proof
of the C26 middle-timestamp gap or of a cheap attack on the live chain.
