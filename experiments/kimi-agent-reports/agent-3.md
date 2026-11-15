# agent-3: Fact-check earlier review

(A final message from the kimi session wire; status may be completed OR died mid-lane)

## Bottom line

The **tower/flat-subfield argument and the one-permutation node attack are fundamentally sound**. However, the review contains several opponent-usable overclaims:

1. `TAG_EXSTNOD` and `TAG_SEGMENTTREE` are **not** production users of the vulnerable flat compressor.
2. The \(2^{16.3}\) and \(2^{63.7}\) costs are **random-looking permutation heuristics**, not consequences of pigeonhole alone.
3. Padded sponge finalization **breaks the claimed subfield confinement**.
4. Fixed points are real, but pinned capsule depths block the stated depth-extension route.
5. The QROM table is accurate, but the quoted asymptotic constants are wrong and a vacuous upper bound is not an attack.
6. The results invalidate the **applicability of the ideal-permutation model**, not Theorem 5.3 or Lemma K.1 within their own hypotheses, and do not by themselves produce a practical authorization forgery.

`R` below denotes `/Users/ember/dev/parano1d-break/parano1d`, whose HEAD is the pinned commit `52938433…`; `A` denotes the distributed artifact directory.

## Confirmed, with qualifications

### 1. Tower and flat-basis invariance: confirmed

All 264 round constants lie in `0x0..=0x7`, the full matrix lies in the same range, and all partial-matrix entries are below \(2^{16}\):

- `A/source/noid_poseidon2b/src/native/permutation.rs:184-206`

The tower implementation embeds smaller fields by zero-extending the raw representation; for example, `Block64` embeds into `Block128` without changing its low 64 bits:

- `R/noid_core/src/tower/block128.rs:192-205`

Consequently, every constant and matrix entry belongs to tower \(\mathbb F_{2^{16}}\). The initial matrix, constant additions, \(x^7\), and both linear layers therefore preserve \(\mathbb F_{2^{16}}\), \(\mathbb F_{2^{32}}\), and \(\mathbb F_{2^{64}}\).

The flat implementation converts every constant with the field isomorphism `tower_to_flat_u128` and uses flat field multiplication:

- `R/noid_core/src/hardware.rs:498-507`
- `A/source/noid_poseidon2b/src/native/permutation.rs:101-130`

Thus the review’s definition \(V_k=\varphi(\mathbb F_{2^k})\) is correct: these are **images under the basis isomorphism**, not simply low-bit flat integers. The flat permutation preserves \(V_{16}^4,V_{32}^4,V_{64}^4\), and the order-eight Frobenius equivariance over \(\mathbb F_{2^{16}}\) follows because all constants and coefficients are fixed by that Frobenius map.

### 2. Node-collision domain and \(2^{16.3}\): algebra confirmed, cost heuristic

For
\[
N_{\rm tag}(a,b)=\operatorname{rate}P(a,b\oplus IV_{\rm tag})\oplus a,
\]
implemented at `A/source/noid_poseidon2b/src/native/compression.rs:327-341`, choose
\[
a\in V_{16}^2,\qquad b=IV_{\rm tag}\oplus v,\quad v\in V_{16}^2.
\]
There are \(2^{64}\) such input pairs, and every output lies in \(V_{16}^2\), which has size \(2^{32}\).

Therefore:

- At most \(2^{32}\) outputs are possible on this domain.
- A collision is guaranteed after at most \(2^{32}+1\) evaluations.
- **If** the restricted map behaves like a random mapping, expected birthday work is
  \[
  \sqrt{\pi/2}\;2^{16}=82{,}137.2=2^{16.3257}
  \]
  evaluations.

Pigeonhole alone does **not** prove the \(2^{16.3}\) figure, uniform fiber sizes, or surjectivity onto all \(2^{32}\) outputs. The wall-clock “milliseconds” claim is also not established by the artifact.

**Strongest accurate wording:**  
“On an efficiently sampleable \(2^{64}\)-input domain, the flat node compressor has at most \(2^{32}\) outputs. Under the standard random-mapping birthday heuristic, this yields chosen-input collisions in approximately \(2^{16.33}\) evaluations.”

### 3. Cross-tag translation: identity confirmed, production scope overstated

The identity
\[
N_{t_2}(a,b\oplus IV_{t_1}\oplus IV_{t_2})=N_{t_1}(a,b)
\]
is immediate from `compression.rs:333-339`. It transfers a collision with two right-child XORs.

But the claimed translation into the exact UTXO tree is incorrect:

- The benchmark artificially calls the flat compressor with `TAG_EXSTNOD`: `A/benchmarks/src/bin/native-node-model.rs:116-120`.
- Production `state_node_hash` instead calls the distinct **two-permutation** `compress_with_tag`: `R/noid_chain/src/exact_state_hash.rs:31-34`.
- The recursive paired-update circuit also uses two permutations per compression: `R/noid_recursive/src/acceptance/trace/paired_merkle_update.rs:217-249`.

`TAG_SEGMENTTREE` has no production call site in the pinned repository beyond its declaration/re-export. The segmented-state implementation uses the generic two-permutation `compress`, e.g. `R/noid_chain/src/segmented_state.rs:258-264`.

Valid actual targets of the identity include other users of the **same** flat compressor, notably:

- `TAG_CAPSNODE`: `R/noid_fri_binius/src/capsule.rs:176-200`
- `IVCPCSN_`: `R/noid_ivc_core/src/merkle.rs:122-129`

The identity is independent of the subfield flaw and survives changing round constants. It still does not automatically yield a semantic cross-protocol exploit.

### 4. Fixed points: confirmed; depth caveat is essential

The inverse exponent in `A/benchmarks/src/bin/native-node-model.rs:65-77` is correct, and for any \(z\),
\[
(a,c)=P^{-1}(0,0,z_0,z_1),\qquad b=c\oplus IV_{\rm tag}
\]
gives \(N_{\rm tag}(a,b)=a\). This is one **inverse-permutation evaluation**, not one ordinary forward hash evaluation. The artifact checks 32 examples, and the algebra proves the general statement.

This property holds for any publicly invertible permutation; regenerating constants does not remove it.

It enables repeated depth extension only where path length is not fixed. The capsule pins source and mid path depths to 10 and 6:

- `R/noid_fri_binius/src/zk_capsule_pcs.rs:63-79`
- `R/noid_fri_binius/src/zk_capsule_pcs.rs:115-116`

Verification iterates exactly the derived depth and rejects unused siblings:

- `R/noid_fri_binius/src/interleaved_commit.rs:339-399`

Thus no capsule depth-extension attack has been demonstrated.

## Quantum sponge claim: substantial correction

For an **aligned, no-padding** schedule, the attack reasoning is coherent:

1. Search the first absorbed block for capacity in \(V_{64}^2\). Under a random-looking output-density assumption, the marked fraction is \(2^{-128}\), giving Grover cost
   \[
   (\pi/4)2^{64}=2^{63.6515}.
   \]
2. Choose the next block so the complete permutation input lies in \(V_{64}^4\).
3. Subsequent no-pad outputs lie in \(V_{64}^2\), of size \(2^{128}\); BHT then costs about \(2^{42.67}\), negligible beside the Grover stage.

So \(2^{63.7}\) is a reasonable **heuristic quantum-query estimate**, approximately 21.68 bits below generic \(2^{85.33}\) BHT collision search. Subfield membership alone does not prove the required marked-point density.

The flaw applies to fixed no-pad leaf hashes such as:

- `TAG_CAPSMIX`, `TAG_CAPS256`: `A/source/noid_fri_binius/src/capsule.rs:395-422`

and to suitably aligned duplex/squeeze schedules.

It does **not** apply as written to padded `finalize()` hashing, including `TAG_BYTEHASH`. `fill_padding` always places `0x01` at byte 31:

- `A/source/noid_poseidon2b/src/native/compression.rs:292-298`

That is bit 120 of the second 128-bit lane, outside tower \(\mathbb F_{2^{64}}\). Therefore the statement that “padded finalization remains confined” is false. The untagged \(2^{16.3}\) classical-sponge claim likewise needs a no-pad or aligned-squeeze restriction.

**Strongest accurate wording:**  
“For tagged multi-block sponges used in aligned no-pad or unpadded-duplex schedules, a random-looking-permutation heuristic gives collisions in approximately \(2^{63.7}\) quantum queries. This estimate does not cover the padded `finalize()` construction without additional analysis.”

## QROM numbers: table confirmed, interpretation and asymptotics need correction

The table matches `A/data/security-bounds.json` to rounding:

- \(T=2^{16}\): 99.97 bits for \(S=1\), 83.97 for \(S=T\).
- \(T=2^{32}\): 44.96 and 41.65 bits.
- At \(T=2^{40}\), the residual bounds are approximately \(2^{-4.965}\) and \(2^{-1.648}\).
- At \(T=2^{41}\), extraction is clipped at 1 and the combined bound is vacuous.

But direct evaluation of `closed_bound` gives
\[
\texttt{causal\_union\_30}\approx T^5\,2^{-204.965}
\]
at high \(T\), not \(\Theta(T^5/2^{221}+T^4/2^{196})\). The degree-five degradation and roughly \(2^{41}\) vacuity point are right; the quoted denominators and additional \(T^4\) term should be removed.

Most importantly, these are upper bounds on
\(\varepsilon_{\rm sim}+\varepsilon_{\rm ext}\), excluding address-preimage advantage and semantic-intent binding error:

- `A/checks/check_security_bounds.py:118-133`
- `A/data/security-bounds.json:61-73`
- `A/manuscript/sections/05-security.tex:116-137`

\(T\) includes adversary, simulator, verifier, constructor, recheck, inverse, and nonce-search base calls—not merely adversary hash queries. A bound becoming vacuous is a **loss of provable reduction security**, not evidence of a \(2^{41}\)-query forgery. The commodity-timing and universal “NIST Level-1 \(2^{128}\) post-quantum query budget” statements should also be removed.

**Strongest accurate wording:**  
“The artifact’s own ideal-model reduction-loss upper bound degrades at roughly fifth degree in total charged base calls, provides only about 41.65 bits of sim-plus-ext residual at \(T=S=2^{32}\), and becomes vacuous near \(2^{41}\). This is a weakness of the proved bound, not a demonstrated forgery algorithm.”

## Theorem falsification versus practical forgery

The review should not say that Theorem 5.3 or Lemma K.1 is mathematically falsified:

- Theorem 5.3 explicitly assumes a uniform random completion: `A/manuscript/sections/05-security.tex:96-128`.
- Lemma K.1 explicitly reasons on a **guarded database**; its guard excludes two distinct reachable edges with the same endpoint: `A/manuscript/sections/k-native-extraction.tex:27-45,60-90`.

The shipped permutation’s invariant subspaces and Frobenius symmetry show that it is not justified to treat it as a typical uniform completion. Collisions can make the guard fail, but do not contradict the guarded lemma. Nor is the benchmark’s `nodes.insert(...).is_none()` assertion a complete statement of the theorem’s assumptions.

No reviewed result recovers an address preimage, produces an accepted fresh-intent capsule without the secret, or gives a second preimage against an existing committed tree. The node attack gives chosen-input primitive collisions; the sponge attack gives collisions for selected schedules; the QROM calculation only weakens an upper bound.

**Recommended overall conclusion:**  
“The shipped constants create exact nested-subfield invariants that undermine the random-completion justification and yield practical chosen-input collisions in the actual flat feed-forward compressor. These are serious primitive- and model-applicability failures. They should not yet be presented as a standard-model logical refutation of the guarded extraction lemmas, a second-preimage attack on existing commitments, or a practical authorization forgery.”

Finally, the citation in review §2.1 is inaccurate: current [ePrint 2025/1893, §3.2.4](https://eprint.iacr.org/2025/1893.pdf) specifies **Turbo-SHAKE-256**, not Grain-LFSR constants. Moreover, the artifact’s small constants match the authors’ [legacy Binius reference](https://raw.githubusercontent.com/Poseidon-Hash/Poseidon2b/7072a9438cf48f1b38ad0092cda5452626914068/binius_poseidon2b/crates/circuits/src/hades/poseidon2b_x7_128_512.rs), so the attack should not be portrayed as breaking the paper’s freshly generated Turbo-SHAKE instance. Full-width round constants plus renewed security analysis are a sensible repair, but requiring **every** constant to generate \(\mathbb F_{2^{128}}\) and re-deriving the MDS matrices is stronger than necessary merely to destroy these invariant subspaces.
