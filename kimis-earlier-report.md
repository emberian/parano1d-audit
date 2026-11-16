# Post-Quantum "Signatureless" Authorization, Capability Delegation, and Threshold Custody: A Cryptanalytic & Architectural Review

**Date:** November 15, 2026  
**Primary Subject:** Ignotus Nemo, *"Post-Quantum Signatureless Authorization"* ([Zenodo Record 23778742](https://zenodo.org/records/23778742), artifact commit `52938433a30b185ab007afaa15166a632cf2e82e`)  
**Comparative Systems:** [`emberian/qsag`](https://github.com/emberian/qsag), [`emberian/dregg`](https://github.com/emberian/dregg), and [`emberian/zkml-research`](https://github.com/emberian/zkml-research)

---

## Executive Summary

Zenodo Record `23778742` introduces a transaction authorization protocol billed as *"Post-Quantum Signatureless Authorization"* (*"No signatures. Proof-native authorization."*). Inspection of the manuscript (`01-ownership.tex` through Appendix K) and the distributed Rust source (`noid_poseidon2b`, `noid_gkr`, `noid_fri_binius`, `noid_tx`) establishes four technical conclusions:

1. **Taxonomic Equivalence (`[CL06]` Signature of Knowledge / `Picnic` Class):** The construction is not "signatureless." Formally, $(\mathsf{KeyGen}, \mathsf{Authorize}, \mathsf{Verify})$ is a **stateless randomized NIZK Signature of Knowledge** in the sense of Chase and Lysyanskaya (`[CL06]`) for the one-way preimage relation $\mathcal{R}_{\text{own}}(a, s) \iff \operatorname{first}_2 P(s_0, s_1, \iota_0, \iota_1) = a$ over a 256-bit static secret key $s \in \mathbb{F}_{2^{128}}^2$, bound to message digest $m$ via the Fiat–Shamir heuristic. It occupies the same primitive class as **`Picnic` (`[CDG+17]`)** and **`FAEST` (`[BBd+23]`)** (Fiat–Shamir NIZKPoK of a symmetric preimage), replacing MPC-in-the-head / VOLE with a binary-tower GKR sumcheck and affine BaseFold/FRI-Binius polynomial commitment (`88.8–90.2 KB` wire signature, `814–894 ms` 4-thread signing, `25.6 ms` standalone verification).
2. **Algebraic Subfield Failure in `Poseidon2b` ($\mathbb{F}_{2^{16}}$ Confinement, $2^{16.3}$ Classical Merkle Collisions, $2^{63.7}$ Quantum Sponge Collisions):** In `noid_poseidon2b/src/native/permutation.rs`, all $264$ entries of `ROUND_CONSTANTS` lie in `{0x0..0x7}` $\subset \mathbb{F}_{2^4}$, all entries of `MDS_FULL` lie in `{0x1..0x7}` $\subset \mathbb{F}_{2^4}$, and all entries of `MDS_PARTIAL` lie in `{0x1, 0x20, 0x200, 0x800, 0x2000}` $\subset \mathbb{F}_{2^{16}}$. Because $x \mapsto x^7$ preserves every subfield of $\mathbb{F}_{2^{128}}$, the entire 66-round permutation is closed on $(\mathbb{F}_{2^{16}})^4$, $(\mathbb{F}_{2^{32}})^4$, and $(\mathbb{F}_{2^{64}})^4$, and commutes with the order-8 Frobenius group $\operatorname{Gal}(\mathbb{F}_{2^{128}}/\mathbb{F}_{2^{16}})$. In the feed-forward Merkle node compressor `compress_flat_feed_forward_with_tag` (`TAG_CAPSNODE`, `TAG_EXSTNOD`, `TAG_SEGMENTTREE`), the tag IV XORs directly into the right child input prior to the single permutation call. Cancelling the IV on subfield-aligned inputs restricts $2^{64}$ inputs to $2^{32}$ possible 256-bit digests, producing **classical 256-bit Merkle internal-node collisions in $\approx 2^{16.3}$ ($\sim 8.2 \times 10^4$) evaluations** and falsifying the *"Address-Compatible Random-Completion Model"* of Theorem 5.3 and Lemma K.1. For multi-block tagged sponges (`TAG_CAPSMIX`, `TAG_CAPS256`, `TAG_BYTEHASH`, `TAG_KSCH256`), a single Grover search into the $(\mathbb{F}_{2^{64}})^2$ capacity subspace followed by BHT yields **256-bit quantum sponge collisions in $\approx 2^{63.7}$ queries** (over 21 bits below the $2^{85.3}$ generic post-quantum collision bound).
3. **Proved QROM Soundness Collapse at $T \approx 2^{41}$ Queries:** Even under an idealized random permutation, the artifact's own rational bound verifier (`checks/check_security_bounds.py`, `data/security-bounds.json`) exhibits degree-5 query degradation $\varepsilon_{\text{ext}}^{\text{Q}} = \Theta(T^5 / 2^{221} + T^4 / 2^{196})$. Proved security never reaches 128 bits even at $T = 2^{16}$ ($84.0$ bits for $S=T$), drops to **$41.6$ bits** at $T = 2^{32}$ ($\sim 4.3 \times 10^9$ hash calls), and becomes **vacuous ($\varepsilon \ge 1$, zero proved bits)** around $T \approx 2^{41}$ — 87 bits short of a NIST Level-1 post-quantum query budget.
4. **Protocol, Multi-Party, and Capability Tradeoffs:** Omitting `chain_id` from `TxBody::hash()` creates cross-fork replay exposure where `epoch_anchor` histories overlap; excluding execution fills from `TradeIntent::authorization_id()` licenses builders to pin fills to worst-case user limits (`minimum_output`, `fee_limit`); and the proposed multi-party extension ($t$ independent 90 KB proofs + committed policy threshold + STARK recursion) is smart-account multisig rather than threshold key custody, contrasting sharply with native MPC threshold signing + WHIR aggregation (`qsag`) and sub-millisecond attenuated capability chains (Macaroons `[BGL+14]`, Biscuit, `dregg`).

---

## 1. Reduction to Standard Cryptographic Primitives

### 1.1 Exact Construction of Zenodo `23778742`

Let $\mathbb{F}_{2^{128}}$ denote the binary tower field (`Block128`) and $\mathbb{F}_{2^{256}} = \mathbb{F}_{2^{128}}[X]/(X^2 + X + \tau)$ its quadratic extension (`Block256`, `noid_core/src/tower/block256.rs`). Let $P : (\mathbb{F}_{2^{128}})^4 \to (\mathbb{F}_{2^{128}})^4$ be the 66-round width-4 `Poseidon2b` permutation ($R_F = 8$ full rounds split $4 + 4$ around $R_P = 58$ partial rounds, S-box $x \mapsto x^7$).

1. **Key Generation (`noid_gkr/src/owner_auth.rs`):**
   Sample a static secret $s = (s_0, s_1) \xleftarrow{\$} \mathbb{F}_{2^{128}}^2$. Fix the public capacity IV $(\iota_0, \iota_1) = \mathsf{IV}(\texttt{"ADDRFIX\_"})$ (`noid_poseidon2b/src/native/domain.rs:56`). The 32-byte public address (`o1...` Bech32m) is the truncated permutation output
   $$a = \mathsf{Addr}(s) := \operatorname{first}_2\!\big(P(s_0, s_1, \iota_0, \iota_1)\big) \in \mathbb{F}_{2^{128}}^2.$$
2. **Authorization / Signing (`noid_gkr/src/zk_authorization.rs`):**
   To authorize an intent digest $m = \mathsf{ID}(I) \in \mathbb{F}_{2^{128}}^2$, `Authorizer::prove` constructs a randomized non-interactive zero-knowledge argument of knowledge $\pi$ (`ZkAuthorizationCapsule`) for the NP relation
   $$\mathcal{R}_{\text{own}}\big((a, m);\; s\big) \iff \operatorname{first}_2\!\big(P(s_0, s_1, \iota_0, \iota_1)\big) = a,$$
   where $(m, a)$ is absorbed into the Fiat–Shamir channel (`TAG_KSCH256` / `TAG_LANE256`) prior to squeezing sumcheck and PCS challenges:
   - **Arithmetization & ZK Sumcheck:** 16 lanes of the 66-round `Poseidon2b` circuit are reduced via degree-10 multilinear sumcheck masked by a Libra univariate blinding polynomial over $\mathbb{F}_{2^{256}}$. Challenges avoid the base subfield $\mathbb{F}_{2^{128}}$ via the 2-to-1 Artin–Schreier map $(y_0, y_1) \mapsto y_0 + (y_1^2 + y_1 + \tau) X$ (`Block256::from_raw_challenge_lanes`, 255 bits of min-entropy).
   - **Polynomial Commitment (`noid_fri_binius/src/zk_capsule_pcs.rs`):** Witness columns and fold layers are committed into two unsalted `Poseidon2b` Merkle trees (`source_tree` of $4{,}096$ mixed leaves; `mid_tree` of $65{,}536$ wide leaves) under an affine Reed–Solomon / BaseFold code ($N_0 = 65{,}536$, rate $\rho = 1/32$, $65$ query openings, 16-bit proof-of-work nonce).
3. **Verification:**
   `Authorizer::verify` enforces canonical fixed-width wire decoding (`<= 92,696` bytes, `zk_authorization_wire.rs`), replays the transcript seeded with `(m, a)`, checks the sumcheck identities and 16-bit PoW, and verifies the 65 Merkle openings against the 8-digest tree caps (`source_cap`, `mid_cap`).

### 1.2 Why This Is a Digital Signature Scheme

Recall two foundational definitions:

- **Digital Signature (`[GMR88]`):** A triple of PPT algorithms $(\mathsf{KeyGen}, \mathsf{Sign}, \mathsf{Verify})$ where $\mathsf{KeyGen}(1^\lambda) \to (pk, sk)$, $\mathsf{Sign}(sk, m) \to \sigma$, and $\mathsf{Verify}(pk, m, \sigma) \to \{0,1\}$, satisfying correctness and EUF-CMA unforgeability against adversaries given $pk$ and a signing oracle $\mathsf{Sign}(sk, \cdot)$.
- **Signature of Knowledge (`[Chase–Lysyanskaya, CRYPTO 2006]`):** A simulation-extractable NIZK proof of knowledge $\pi \leftarrow \mathsf{SoKProve}(\mathcal{R}, x, w, m)$ binding an arbitrary message $m \in \{0,1\}^*$ to witness knowledge $(x, w) \in \mathcal{R}$. As Chase and Lysyanskaya proved (`[CL06, §3]`), instantiating a Signature of Knowledge over any one-way function $f(sk) = pk$ immediately yields an EUF-CMA public-key signature scheme (and vice versa).

Setting $pk = a$, $sk = s$, and $\sigma = \pi$, Zenodo `23778742` is syntactically and semantically a **stateless symmetric-primitive post-quantum signature scheme**:
- Unlike Anderson's **Guy Fawkes protocol** (`[And98]`) or Lamport/WOTS/XMSS forward-commitment chains — which genuinely omit public-key Trapdoor/ZK machinery by rotating one-time hash preimages $h_i = H(x_i)$ across stateful steps — Zenodo `23778742` **reuses a static secret key $s$ across arbitrarily many messages** and prevents $s$ from leaking via a Fiat–Shamir NIZKPoK.
- That is the exact design blueprint of **`Picnic` (`[CDG+17]`, `pk = LowMC_sk(p)`)** and **`FAEST` (`[BBd+23]`, `pk = AES_sk(p)`)** (which the manuscript itself cites in `references.bib`). Calling a STARK-based `Picnic` variant "signatureless" confuses *algebraic proof machinery* with *absence of a signature*.

| Post-Quantum Signature Scheme | Underlying Hardness | Public Key | Signature Size | Sign Time | Verify Time |
| :--- | :--- | ---: | ---: | ---: | ---: |
| `ML-DSA-44` (FIPS 204) | Module-LWE / Module-SIS | 1,312 B | **2,420 B** | ~0.06 ms | ~0.03 ms |
| `ML-DSA-65` (FIPS 204) | Module-LWE / Module-SIS | 1,952 B | **3,309 B** | ~0.10 ms | ~0.05 ms |
| `FAEST-128f` (`[BBd+23]`) | AES-128 OWF + VOLE-itH | 32 B | **5,924 B** | ~0.5 ms | ~0.5 ms |
| `SLH-DSA-128s` (FIPS 205) | SHA-256 / SHAKE256 | 32 B | **7,856 B** | ~90 ms | ~1.5 ms |
| `qsag` `Jorts` (`QS-A` / `QS-B`) | `Poseidon2` / `cSHAKE256` | 32 B | **4,328 B** | 0.56–5.21 ms | < 0.2 ms |
| **Zenodo `23778742` Capsule** | **`Poseidon2b` OWF + STARK** | **32 B** | **88,760–90,168 B** | **814–894 ms** | **25.6 ms** |

Standalone, the capsule pays a **27×–37× wire bloat** and **>8,000× prover latency penalty** relative to lattice signatures, and an **11×–20× wire bloat** relative to stateless/stateful hash signatures. Its sole architectural justification is **STARK-friendly recursion** (`06-recursive.tex`): when thousands of authorizations are recursively aggregated into a block proof over the same binary tower field, verifying an already-algebraic `Poseidon2b` capsule consumes ~16,536 trace rows without arithmetizing Keccak or prime-field NTTs. Whether that recursion dividend survives depends entirely on the soundness of `Poseidon2b` and the QROM security reduction — both of which fail in the shipped artifact.

---

## 2. Algebraic Cryptanalysis of `Poseidon2b`: $\mathbb{F}_{2^{16}}$ Subfield Confinement and Collision Attacks

### 2.1 Subfield Round Constants and Matrices in `noid_poseidon2b`

In Grassi et al. (`[GKK+25]`, *Poseidon(2)b: Binary Field Versions of Poseidon/Poseidon2*, ePrint 2025/1893), binary-field Poseidon2 instances over $\mathbb{F}_{2^n}$ require pseudorandom Grain-LFSR round constants sampled uniformly over the **full extension field $\mathbb{F}_{2^n}$** so that no proper subfield $\mathbb{F}_{2^m} \subsetneq \mathbb{F}_{2^n}$ ($m \mid n$) forms an invariant subspace across rounds.

In `noid_poseidon2b/src/native/permutation.rs` (lines 184–206, bearing the header comment `// Adapted from binius_poseidon2b circuit reference`), the deployed constants are:

```rust
pub const ROUND_CONSTANTS: [[u128; N_ROUNDS]; STATE_SIZE] = [
    [0x0, 0x1, 0x0, 0x3, 0x7, 0x7, 0x6, 0x6, /* ... all 264 entries in 0x0..=0x7 */],
    // ...
];

pub const MDS_FULL: [[u128; STATE_SIZE]; STATE_SIZE] = [
    [0x5, 0x7, 0x1, 0x3],
    [0x4, 0x6, 0x1, 0x1],
    [0x1, 0x3, 0x5, 0x7],
    [0x1, 0x1, 0x4, 0x6],
];

pub const MDS_PARTIAL: [[u128; STATE_SIZE]; STATE_SIZE] = [
    [0x20, 0x00000001, 0x00000001, 0x00000001],
    [0x00000001, 0x2000, 0x00000001, 0x00000001],
    [0x00000001, 0x00000001, 0x200, 0x00000001],
    [0x00000001, 0x00000001, 0x00000001, 0x800],
];
```

In the Wiedemann / Fan–Paar binary tower $\mathbb{F}_2 \subset \mathbb{F}_{2^2} \subset \mathbb{F}_{2^4} \subset \mathbb{F}_{2^8} \subset \mathbb{F}_{2^{16}} \subset \mathbb{F}_{2^{32}} \subset \mathbb{F}_{2^{64}} \subset \mathbb{F}_{2^{128}}$, integer encodings $< 2^{2^k}$ represent the exact tower subfield $\mathbb{F}_{2^{2^k}}$:
- Every entry of `ROUND_CONSTANTS` (`0x0..0x7`) lies in **$\mathbb{F}_{2^4}$**;
- Every entry of `MDS_FULL` (`0x1..0x7`) lies in **$\mathbb{F}_{2^4}$**;
- Every entry of `MDS_PARTIAL` (`0x1`, `0x20`, `0x200`, `0x800`, `0x2000`, all $< 2^{16}$) lies in **$\mathbb{F}_{2^{16}}$ (`Block16`)**;
- Every round operation — XOR with `ROUND_CONSTANTS`, monomial S-box $x \mapsto x^7$, and linear multiplication by `MDS_FULL` or `MDS_PARTIAL` — is defined over $\mathbb{F}_{2^{16}}$.

Let $\varphi : \mathbb{F}_{2^{128}}^{\text{tower}} \xrightarrow{\sim} \mathbb{F}_{2^{128}}^{\text{flat}}$ be the basis isomorphism `tower_to_flat_u128` (`permutation.rs:107-118`), and define the flat subfield images $V_k := \varphi(\mathbb{F}_{2^k})$ for $k \in \{16, 32, 64\}$ ($|V_k| = 2^k$).

**Proposition 1 (Nested Subfield Invariance & Order-8 Galois Symmetry).**
1. *For each $k \in \{16, 32, 64\}$, the 66-round flat permutation `permute_flat_u128` restricts to a bijection $P|_{V_k^4} : V_k^4 \to V_k^4$ of cardinality $2^{4k}$ ($2^{64}$, $2^{128}$, and $2^{256}$, respectively).*
2. *Let $\phi(x) = x^{2^{16}}$ generate $\operatorname{Gal}(\mathbb{F}_{2^{128}} / \mathbb{F}_{2^{16}}) \cong \mathbb{Z}/8\mathbb{Z}$ (acting componentwise on 4-lane states). Because $\operatorname{Fix}(\phi) = \mathbb{F}_{2^{16}}$ contains all round constants and matrix entries of $P$, $P$ is $\operatorname{Gal}(\mathbb{F}_{2^{128}} / \mathbb{F}_{2^{16}})$-equivariant everywhere on $(\mathbb{F}_{2^{128}})^4$:*
   $$P\big(\phi^j(x_0), \phi^j(x_1), \phi^j(x_2), \phi^j(x_3)\big) = \phi^j\!\big(P(x_0, x_1, x_2, x_3)\big) \qquad \forall j \in \{0,\dots,7\}.$$

### 2.2 $2^{16.3}$-Evaluation Classical Collisions in Merkle Node Compression

Every internal binary Merkle node in the capsule PCS (`source_tree` and `mid_tree` under `TAG_CAPSNODE` in `noid_fri_binius/src/capsule.rs:190-202`), the exact UTXO state tree (`TAG_EXSTNOD`), and the multi-segment root tree (`TAG_SEGMENTTREE`) is computed by `compress_flat_feed_forward_with_tag` (`noid_poseidon2b/src/native/compression.rs:327-341`):

$$N_{\text{tag}}(a, b) = \operatorname{rate}\!\Big(P\big(a_0,\; a_1,\; b_0 \oplus \mathsf{iv}_{\text{hi}},\; b_1 \oplus \mathsf{iv}_{\text{lo}}\big)\Big) \oplus (a_0, a_1),$$

where $a = (a_0, a_1)$ and $b = (b_0, b_1)$ are the two 32-byte child digests and $(\mathsf{iv}_{\text{hi}}, \mathsf{iv}_{\text{lo}}) = \texttt{capacity\_iv\_flat}(\text{tag})$.

Because $(\mathsf{iv}_{\text{hi}}, \mathsf{iv}_{\text{lo}})$ is XORed directly into the caller-supplied right child $b$ **before** the single permutation call, the domain tag fails to force the permutation state out of $V_{16}^4$:

- Choose arbitrary 16-bit subfield elements $u_0, u_1, v_0, v_1 \in \mathbb{F}_{2^{16}}$ ($65{,}536$ choices each) and set
  $$a = \big(\varphi(u_0),\; \varphi(u_1)\big) \in V_{16}^2, \qquad b = \big(\mathsf{iv}_{\text{hi}} \oplus \varphi(v_0),\; \mathsf{iv}_{\text{lo}} \oplus \varphi(v_1)\big).$$
- The permutation input is $(\varphi(u_0), \varphi(u_1), \varphi(v_0), \varphi(v_1)) \in V_{16}^4$. By Proposition 1, $P$ maps $V_{16}^4 \to V_{16}^4$, so the 256-bit Merkle parent digest $N_{\text{tag}}(a, b)$ **always lies in $V_{16}^2$ — a 32-bit subspace of size $2^{32} = 4{,}294{,}967{,}296$**.
- **Deterministic Pigeonhole Collisions & $2^{16.3}$ Birthday Cost:** $N_{\text{tag}}$ maps $2^{64}$ distinct child pairs $(a, b)$ into $2^{32}$ digests (average preimage multiplicity $2^{32}$ per digest). Standard birthday sampling finds a full **256-bit Merkle internal-node collision $N_{\text{tag}}(a, b) = N_{\text{tag}}(a', b')$ in $\approx \sqrt{\frac{\pi}{2}} \cdot 2^{16} \approx 82{,}137$ ($2^{16.33}$) evaluations** (milliseconds on a single CPU core, vs. the $2^{128}$ classical / $2^{85.3}$ quantum bound assumed for a 256-bit hash).
- **$O(1)$ Cross-Tree Collision Translation:** Because $N_{\text{tag}_2}(a,\; b \oplus \mathsf{iv}_{\text{tag}_1} \oplus \mathsf{iv}_{\text{tag}_2}) = N_{\text{tag}_1}(a, b)$ (`benchmarks/src/bin/native-node-model.rs:116-120`), any collision found for `TAG_CAPSNODE` immediately yields a valid collision for `TAG_EXSTNOD` (UTXO state tree) and `TAG_SEGMENTTREE` via a single constant XOR on the right child.
- **1-Query Fixed Points:** Because $\gcd(7, 2^{128}-1) = 1$, $x \mapsto x^7$ has explicit inverse exponent $d = \texttt{0x49249249249249249249249249249249}$, allowing $P^{-1}$ to be evaluated directly (`native-node-model.rs:65-77`). For any $z \in (\mathbb{F}_{2^{128}})^2$, $(a, c) = P^{-1}(0, 0, z_0, z_1)$ yields $N_{\text{tag}}(a, c \oplus \mathsf{iv}_{\text{tag}}) = a$ in one query, enabling trivial depth-extension against any Merkle verifier that does not pin path length.

### 2.3 $2^{63.7}$-Query Quantum Collisions on All Multi-Block Tagged Sponges

What about sponge-mode constructions (`Poseidon2bSponge` and `Poseidon2bFlatSponge`, used for `TAG_CAPSMIX`, `TAG_CAPS256`, `TAG_BYTEHASH`, `TAG_CLAIMS`, `TAG_FSCH256`, and `TAG_KSCH256`)?

1. **Untagged Sponge (`Poseidon2bSponge::new()`, `compression.rs:60-66`):** Initializes `state = [0, 0, 0, 0]` $\in V_{16}^4$. Absorbing subfield blocks keeps the entire state in $V_{16}^4$, yielding **$2^{16.3}$-query classical 256-bit collisions**.
2. **All Tagged Multi-Block Sponges (`capacity_iv(tag)`, `domain.rs:35-52`):** For an 8-byte ASCII label $\ell \neq 0$, `capacity_iv` initializes the capacity lanes to $(\mathsf{iv}_{\text{hi}}, \mathsf{iv}_{\text{lo}}) = (\varphi(\ell \ll 64), \varphi(\ell))$. Here $\varphi(\ell) \in V_{64}$, whereas $\varphi(\ell \ll 64) \notin V_{64}$, so the initial state begins outside $V_{64}^4$. However, in any sponge absorbing $\ge 2$ rate blocks $m^{(0)}, m^{(1)} \in (\mathbb{F}_{2^{128}})^2$:
   - After block $0$, $(r^{(0)}, c^{(0)}) = P(m_0^{(0)}, m_1^{(0)}, \mathsf{iv}_{\text{hi}}, \mathsf{iv}_{\text{lo}})$. The event $c^{(0)} \in V_{64}^2$ constrains $256 - 128 = 128$ bits of the 256-bit capacity, which has density $2^{-128}$ over choices of $m^{(0)} \in (\mathbb{F}_{2^{128}})^2$.
   - A quantum adversary finds an $m^{(0)}$ with $c^{(0)} \in V_{64}^2$ via Grover's algorithm in $\frac{\pi}{4}\sqrt{2^{128}} \approx 2^{63.65}$ quantum queries.
   - Once $c^{(0)} \in V_{64}^2$, choosing $m^{(1)} = r^{(0)} \oplus u$ with $u \in V_{64}^2$ places the full 512-bit state $(u_0, u_1, c_0^{(0)}, c_1^{(0)})$ inside the invariant subspace $V_{64}^4$! Every subsequent round and padded finalization remains confined to $V_{64}^4$, restricting the 256-bit sponge digest to the 128-bit subspace $V_{64}^2$.
   - Finding a collision inside $V_{64}^2$ requires only $2^{64}$ classical queries or $2^{128/3} \approx 2^{42.7}$ quantum BHT queries — giving **full 256-bit quantum collisions on every multi-block `Poseidon2b` sponge in $\approx 2^{63.7}$ quantum queries** (beating the generic $2^{85.3}$ BHT bound by **21.6 bits**, or $> 3 \times 10^6\times$).

### 2.4 Falsification of Theorem 5.3 and Appendix K's Idealized Oracle Model

Theorem 5.3 (`05-security.tex:96-135`) and Appendix K (`k-native-extraction.tex:60-90`) do not prove security in the standard model or over `Poseidon2b` directly; instead, they assume an *"Address-Compatible Random-Completion Model"* in which $P$ outside the fixed $2^{256}$-element `ADDRFIX_` capacity slice is modeled as a uniform random permutation on $2^{512} - 2^{256}$ elements. Under that model, Lemma K.1 (*Typed native reconstruction*) relies on node-digest uniqueness (`assert!(nodes.insert(digest, children).is_none(), "node collision")` in `benchmarks/src/bin/native-source-extraction.rs:157`) holding up to $O(T^2 / 2^{256})$.

Because $V_{16}^4$ is strictly disjoint from the `ADDRFIX_` slice ($\mathsf{iv}_{\text{hi}} \notin V_{64} \supset V_{16}$) yet exhibits $2^{32}$-to-$1$ node collisions in $2^{16.3}$ queries and global order-8 Frobenius symmetry across all $2^{512}$ states, **the oracle assumption underlying Theorem 5.3 is structurally violated by the shipped `Poseidon2b` permutation**. Fixing this requires regenerating full-width $\mathbb{F}_{2^{128}}$ round constants (each verified to generate $\mathbb{F}_{2^{128}}$ over $\mathbb{F}_2$, outside $\mathbb{F}_{2^{64}}$) and re-deriving MDS parameters, breaking wire/address compatibility with all existing artifacts.

---

## 3. QROM Soundness Collapse & Protocol Edge Audit

### 3.1 Degree-5 $O(T^5)$ QROM Extraction Loss (`security-bounds.json`)

To the author's credit, the artifact includes an exact 120-digit rational script (`checks/check_security_bounds.py`) evaluating the closed-form QROM/QRPM simulation and extraction bounds of Appendices F, J, and K (`data/security-bounds.json`). Inspecting the numbers produced by that script reveals that **even if `Poseidon2b` were an ideal permutation, the proved post-quantum security bound collapses at small query budgets**:

| Total Oracle Queries $T$ | Signing Queries $S$ | Proved $\varepsilon_{\text{sim}} + \varepsilon_{\text{ext}}$ (`security-bounds.json`) | Proved Security ($-\log_2 \varepsilon$) |
| :--- | :--- | :--- | ---: |
| $2^{16} = 65{,}536$ | $1$ / $2^{16}$ | $8.04 \times 10^{-31}$ / $5.27 \times 10^{-26}$ | **99.97 bits** / **83.97 bits** |
| $2^{20} = 1{,}048{,}576$ | $1$ / $2^{20}$ | $1.27 \times 10^{-29}$ / $1.33 \times 10^{-23}$ | **95.99 bits** / **75.99 bits** |
| $2^{24} = 16{,}777{,}216$ | $1$ / $2^{24}$ | $2.71 \times 10^{-26}$ / $3.40 \times 10^{-21}$ | **84.93 bits** / **67.99 bits** |
| $2^{28} = 268{,}435{,}456$ | $1$ / $2^{28}$ | $2.79 \times 10^{-20}$ / $1.15 \times 10^{-18}$ | **64.96 bits** / **59.60 bits** |
| $2^{32} = 4{,}294{,}967{,}296$ | $1$ / $2^{32}$ | $2.91 \times 10^{-14}$ / $2.91 \times 10^{-13}$ | **44.96 bits** / **41.65 bits** |
| **$T \approx 2^{41}$ ($\sim 2.2 \times 10^{12}$)** | **$1$** | **$\ge 1.0$ (Vacuous)** | **0 bits** |

Two structural issues drive this degradation:
1. **$O(T^5)$ Extraction Loss (`j-fresh-extraction.tex`):** Squaring `(T + l) * eps_T + B_pref` inside the per-move `hit`/`miss` terms and multiplying by `(8T + 1)(4T + 1)` across the 30-move causal union produces $\Theta(T^5 / 2^{221} + T^4 / 2^{196})$ scaling — losing **20 bits of proved security for every $16\times$ ($2^4$) increase in adversary queries**. At $T = 2^{32}$ (sub-second hash evaluation on commodity hardware), only $41.65$ bits remain; by $T \approx 2^{41}$ (87 bits below $2^{128}$), the bound exceeds $1$. By contrast, tight QROM Fiat–Shamir analyses for interactive oracle proofs (e.g., `[CMS19]`, `[CDHZ25]`) require salted commitments and round-by-round soundness to avoid high-degree $T^k$ blowups.
2. **Unsalted, Position-Untagged Merkle Leaves (`capsule.rs:399-423`):** Neither `capsule_leaf_hash_mixed` nor `capsule_leaf_hash_wide` hashes a per-leaf random salt or the leaf index `idx`. Consequently, a prover choosing the final rate pair of a mixed leaf to cancel the running sponge rate (`state[0..2] == [0, 0]`) produces a leaf digest algebraically identical to an internal `TAG_CAPSNODE` digest (`benchmarks/src/bin/native-source-extraction.rs:60-80`), forcing the complex surgery of Appendices F and K.

### 3.2 Malleability, Cross-Chain Replay, and Intent Slippage MEV

1. **Third-Party Wire Non-Malleability vs. Signer Proof Non-Uniqueness:**
   `zk_authorization_wire.rs` enforces canonical fixed-integer `bincode` serialization (`<= 92,696` B) with trailing-byte rejection, and `(m, a)` is absorbed before squeezing Fiat–Shamir challenges. Assuming hash binding (once `Poseidon2b` constants are repaired), a mempool observer without $s$ cannot maul $(m, \pi) \to (m', \pi')$ for $m' \neq m$. Because proving uses fresh `OsRng` randomness, however, any holder of $s$ can generate arbitrarily many distinct valid capsules $\pi \neq \pi'$ for the same $(a, m)$; ledger transaction IDs must therefore hash `TxBody` rather than witness capsules.
2. **Missing `chain_id` in `TxBody::hash()` (`noid_tx/src/owner_auth.rs:63-71`):**
   While `TradePolicy` and `NativeIntent` (`noid_tx/src/native_assets/trade.rs:50-85`) explicitly bind `chain: Digest`, `TxBody::hash()` and `PagedSpend` omit `chain_id`, relying solely on `epoch_anchor` divergence. Across any hard fork or state-snapshot testnet sharing historical `epoch_anchor` roots, `TxBody` capsules replay cleanly unless consensus rules inject fork-specific anchors immediately.
3. **Intent Execution-Witness Split & Builder MEV (`noid_tx/src/native_assets/trade.rs:266-317`):**
   In `TradeIntent`, the user's capsule signs `TradeIntent::authorization_id()`, binding `gross_input`, `minimum_output`, and `fee_limit`, while leaving actual execution fields (`spent_input`, `output`, `protocol_fee`) outside the user's authorization proof so block builders can clear order batches without re-invoking each user's prover. Without an enforced uniform batch-clearing price invariant in the outer block circuit, any builder or MEV searcher is cryptographically authorized to set `output = minimum_output` and `protocol_fee = fee_limit`, extracting 100% of limit surplus.

---

## 4. Multi-Party Threshold Custody vs. Account-Abstraction Multisig

In public discussion of multi-party extensions, the proposed threshold pattern is:

> *"Each participant independently proves authorization for the same request and a committed policy enforces a threshold such as 2-of-3 across distinct participants... Recursive aggregation could then package those approvals into a single proof."*

Architecturally, it is important to distinguish **policy-level $t$-of-$n$ multisig with STARK recursion** from **cryptographic threshold custody** (as implemented in [`emberian/qsag`](https://github.com/emberian/qsag)):

1. **Single-Secret Splitting vs. $n$ Independent Secrets:**
   In true threshold custody (`qsag-quorum` `QS-A`/`QS-B`, `qsag-core` `Baggy`, `qsag-cred` `ThresholdMlDsa`), no single party ever holds a complete signing key, yet the distributed quorum produces **one standard single-key signature** (`4,328 B` `Jorts` or `3,309 B` `ML-DSA-65`) under a **single unchanged 32-byte public address** (`KeyIdentityKind::SingleOrThreshold` in `qsag-addr`). Under the Zenodo `23778742` multisig proposal, each participant holds an independent secret $s_i$; splitting a *single* secret $s$ behind `Addr(s)` would instead require executing a malicious-secure collaborative MPC-STARK prover over 66-round `Poseidon2b`, Reed–Solomon encoding, and Merkle commitments.
2. **Proactive Resharing & Quorum Switching Without Address Churn:**
   When an organization rotates device shares or transitions from `2-of-3` to `3-of-5`, `qsag-quorum` runs dealerless Shamir resharing (`refresh_bft_shamir_shares`) behind the *identical* public key, using a BFT monotone `BeltLatch` ($2q > n + f$) to prevent split-view stateful leaf reuse across asynchronous partitions. Under committed-policy multisig, rotating a compromised participant key $a_i = \mathsf{Addr}(s_i)$ or changing $(t, n)$ mutates the committed policy root, either changing the account's public address or requiring stateful on-chain key-rotation transactions.
3. **Concrete Compute & Bandwidth Scaling:**
   Collecting $t$ independent capsules requires $t \times 890\text{ ms}$ of client prover CPU, $t \times 90\text{ KB}$ of coordinator ingress, and a heavyweight recursive STARK pass (`~16,536 * t` rows) to compress them. By contrast:
   - `qsag-quorum` signs a 3-of-5 threshold `Jorts` authorization in **`0.56 ms` compute (`4.37 ms` durable WAL, `5.21 ms` BFT 3-of-7 `QS-B`)** with a **`4,328 B`** output;
   - `qsag-policy` enforces recipient allowlists, single-tx and rolling-window velocity caps, anti-self-approval, and hardware-guardian escalation inside pairwise **Eurocrypt-2011 `BDOZ` $\mathbb{F}_{2^{128}}$ MAC barriers** *before* any custodian releases a partial signature share;
   - When validator-scale batch compression is needed, `qsag-agg` (`Hem`) non-interactively aggregates $K = 32..128$ post-quantum signatures into a single `GoldilocksExt4` WHIR proof in **`0.32–1.34 s` GPU** (`3.3–7.0 ms` verification) — achieving succinct batch verification without burdening leaf signers with 90 KB STARK proofs.

---

## 5. Signature/ZKPoK Authorization vs. Attenuated Capability Tokens

A deeper architectural lesson emerges when contrasting signature-style request authorization (both classical/PQ signatures and Zenodo `23778742`'s STARK-of-knowledge) against **attenuated capability systems** — Macaroons (`[BGL+14]`), Biscuit Datalog tokens, and their post-quantum/ZK evolutions in [`emberian/dregg`](https://github.com/emberian/dregg) and [`emberian/zkml-research`](https://github.com/emberian/zkml-research).

### 5.1 Ambient Authority Signers vs. Offline Monotone Attenuation

In any signature or STARK-of-knowledge model (`Sign(sk, m)` or `Authorize(s, m)`), holding `s` confers **ambient authority** over the account:
- If an owner wishes to delegate temporary, restricted authority to an agent, worker, or microservice (*"spend at most 500 units to recipient $R$ within 120 seconds, only if risk-oracle $G$ cosigns"*), the owner cannot hand over $s$. Either the owner must stay online as a signing oracle, or the ledger must support complex on-chain policy registries.
- **Attenuated capabilities invert this model:** authority is carried *by the token*, and **any holder can offline-narrow (attenuate) a capability in $O(1)$ time without contacting the issuer or holding a root key**, while no holder can widen authority or strip prior caveats.

### 5.2 Three Concrete Capability Architectures in `dregg`

`emberian/dregg` explores the capability design space across three threat models, bridged by canonical Datalog and zero-knowledge folding:

1. **Symmetric Chained-MAC Capabilities + Bound 3P Discharges (`macaroon/`, `token/`):**
   - Starting from $T_0 = \mathsf{HMAC}(k_{\text{root}}, \text{nonce})$, each caveat $C_i$ updates $T_i = \mathsf{HMAC}(T_{i-1}, \mathsf{encode}(C_i))$ (`~0.5 μs` verification, `~150–400 B` wire).
   - **Third-Party Caveats (`[BGL+14]`):** To require an external gateway $G$ (e.g., MFA, compliance, or payment proof) without online coordination between the root verifier and $G$, the attenuator samples a fresh 32-byte `discharge_key` $r$, embeds $\mathsf{VID} = \mathsf{AEAD}(T_{i-1}, r)$ in the token and $\mathsf{Ticket} = \mathsf{AEAD}(K_G, (r, \text{conditions}))$ for $G$. When $G$ issues a discharge macaroon rooted at $r$, the holder binds it to the parent token's final tail via `CAV_BIND_TO_PARENT = H(T_n)`, preventing discharge replay against less-attenuated prefixes.
   - **Post-Quantum Note:** Chained 256-bit `HMAC-SHA256` or `cSHAKE256`/`KMAC256` + 256-bit AEAD is **already post-quantum secure** (128-bit Grover resistance, zero lattice or algebraic-hash assumptions) at **0.3% of the wire size and 0.0001% of the latency** of a 90 KB STARK capsule whenever the root issuer and verifier share a symmetric key (e.g., API gateways, microservice meshes, or custody-guard session tokens).

2. **Public-Key Offline Capabilities: Hybrid `Ed25519 ∧ ML-DSA-65` Chains (`dregg-auth/`):**
   - When third-party verifiers cannot hold $k_{\text{root}}$ (Biscuit's public-key setting), `dregg-auth` (`dga1_` / `dgd1_`, backed by Lean 4 proofs in `Dregg2.Authority.*`) chains ephemeral hybrid signatures anchored at an enrolled `HybridRootPublic`:
     - Block $i$ signs `BLAKE3("dregg-auth v1 block", prev_sig_or_nonce || caveats_i || next_pub_ed25519 || next_pub_ml_dsa)` under **both** parent keys, pinning the next holder's 1,952-byte ML-DSA-65 public key inside the authenticated chain so PQ keys can never be self-asserted.
     - Presentation requires tail proof-of-possession (preventing trailing-block stripping even against a quantum adversary who breaks Ed25519), with **DoS-ordered verification gates**: classical PoP and full hybrid chain verification execute *before* deriving the tail ML-DSA public key from its carried seed (`174–227 ms` at the verified FFI boundary), ensuring unauthenticated traffic dies in microseconds.
     - Caveats evaluate in a **fail-closed three-valued algebra (`Pred`)** where missing context attributes return `Err(Unbound)` rather than `false`, ensuring negation (`Not`) never amplifies missing environment data into positive authority, paired with $O(\log N)$ sorted-neighbor BLAKE3 Merkle non-membership revocation proofs (`SortedRevocationTree`) under hybrid-signed roots.

3. **Zero-Knowledge Capability Folding & Common-Point Linear Openings (`bridge/`, `fold-opening/`):**
   - When capabilities must be presented **privately** (hiding the delegation chain and attenuators) or compressed across hops, `dregg`'s `bridge` models each attenuation $T_k \to T_{k+1}$ as a monotone Merkle `FoldDelta` (permitting only removal of `unrestricted` facts and insertion of allowlisted `check*` predicates) and proves valid presentation + 4-ary blinded federation ring-membership in Plonky3 BabyBear IR-v2.
   - Complementarily, `fold-opening` (`zkml-research/notes/fold-as-opening.md`) demonstrates when linear operations genuinely avoid circuit overhead: certifying $c_{\text{out}} = \sum_{k=0}^{B-1} a_k c_k$ over committed multilinear vectors requires **zero sumcheck rounds** — just one extension-field evaluation $\hat{c}_{\text{out}}(r) = \sum_k a_k \hat{c}_k(r)$ ($r \in \mathbb{F}_{p^4}^\mu$, soundness $16/2^{124} \approx 2^{-120}$) plus **one** $O(N \cdot L)$ canonicalization range check on the output (amortizing $B$ reductions into $1$), provided all commitments and weights $a_k$ are absorbed into the Fiat–Shamir transcript before squeezing $r$.

### 5.3 High-Leverage Synthesis: Few-Time Ephemeral PQ Biscuit Chains (`Compact1122`)

Comparing `dregg-auth`'s hybrid Biscuit chain (`5,261 B` per hop for `ML-DSA-65`: `1,952 B` pubkey + `3,309 B` signature) with `qsag`'s `qsag-fts` crate exposes a clean structural optimization for post-quantum public-key capabilities:

- **Observation:** In a Biscuit / `dga1_` capability chain, the root authority signs many tokens, **but every intermediate ephemeral block key $k_1, k_2, \dots, k_m$ signs at most once** (either to append one attenuation block or to prove tail possession at presentation). Paying `5,261 B` and lattice keygen cost at every hop buys unlimited signing capacity that an ephemeral capability hop never uses.
- **Construction:** Anchor block 0 at a threshold MPC root (`Jorts` or `ThresholdMlDsa` behind a 32-byte `QsagAddress`), and instantiate every ephemeral attenuation hop $i \ge 1$ with **`FrontierFtsProfile::Compact1122`** (`qsag-fts`, pure-hash few-time signature):
  - Block $i-1$ pins a **32-byte** `next_hop_pk_root` (instead of a 1,952 B ML-DSA-65 public key);
  - Hop $i$ signs `(prev_digest || caveats_i || next_hop_pk_root)` with a **`1,122 B`** `Compact1122` signature.
- **Result:** **`1,154 B` per post-quantum attenuation hop (`4.56×` smaller than ML-DSA-65 capability blocks and `78×` smaller than a 90 KB STARK capsule)**, sub-millisecond keygen and verification, zero lattice assumptions on delegation hops, and direct compatibility with `qsag-agg` (`Hem` GoldilocksExt4 WHIR) whenever many capability presentations + `SortedRevocationTree` non-membership witnesses need succinct batch verification.

---

## References

- **`[And98]`** R. Anderson, F. Bergadano, B. Crispo, J.-H. Lee, C. Manifavas, R. Needham, *"A New Family of Authentication Protocols"* (Guy Fawkes Protocol), ACM Operating Systems Review 32(4), 1998.
- **`[BBd+23]`** C. Baum et al., *"FAEST: Rethinking the AES-based Signature Scheme fromVOLE-in-the-Head,"* CRYPTO 2023 / ePrint 2023/696.
- **`[BGL+14]`** A. Birgisson, J. G. Politz, Ú. Erlingsson, A. Taly, M. Vrable, M. Lentczner, *"Macaroons: Cookies with Contextual Caveats for Decentralized Authorization in the Cloud,"* NDSS 2014.
- **`[CDG+17]`** M. Chase et al., *"Post-Quantum Zero-Knowledge and Signatures from Symmetric-Key Primitives"* (`Picnic`), ACM CCS 2017 / ePrint 2017/279.
- **`[CDHZ25]`** A. Chiesa, E. Dall'Agnol, Z. Hu, E. Zhandry, tight QROM Fiat–Shamir / IOP soundness frameworks; see also `[CMS19]` A. Chiesa, P. Manohar, N. Spooner, *"Succinct Arguments in the Quantum Random Oracle Model,"* TCC 2019 / ePrint 2019/834.
- **`[CL06]`** M. Chase, A. Lysyanskaya, *"On Signatures of Knowledge,"* CRYPTO 2006 / ePrint 2006/184.
- **`[GKK+25]`** L. Grassi, D. Khovratovich, K. Koschatko, C. Rechberger, M. Schofnegger, V. Schröppel, Z. Wu, *"Poseidon(2)b: Binary Field Versions of Poseidon/Poseidon2,"* IACR ePrint 2025/1893.
- **`[GMR88]`** S. Goldwasser, S. Micali, R. L. Rivest, *"A Digital Signature Scheme Secure Against Adaptive Chosen-Message Attacks,"* SIAM J. Comput. 17(2), 1988.
- **Zenodo `23778742`:** Ignotus Nemo, *"Post-Quantum Signatureless Authorization,"* [https://zenodo.org/records/23778742](https://zenodo.org/records/23778742) (2026).
- **Comparative Implementations:** [`emberian/qsag`](https://github.com/emberian/qsag), [`emberian/dregg`](https://github.com/emberian/dregg), [`emberian/zkml-research`](https://github.com/emberian/zkml-research).
