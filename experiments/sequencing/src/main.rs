// SPDX-License-Identifier: Apache-2.0
// Sequencing/ordering audit of the parano1d L1 at pin 52938433.
//
// Every experiment below calls production code from the pinned checkout
// (noid_chain, noid_tx): the mempool fee index, the block template builder,
// state application, the trade funding-priority lottery, the NFT listing
// round/payer-priority lottery, and the deterministic fee model. Nothing here
// starts a node, peer, miner, or RPC server; everything is in-process.

use noid_chain::consensus::fee_breakdown;
use noid_chain::consensus::genesis::genesis_header;
use noid_chain::consensus::params::{BLOCK_MAX_USER_PAGES, LOG_SLOTS_GENESIS};
use noid_chain::consensus::pow::block_id;
use noid_chain::consensus::template::build_block_template;
use noid_chain::state::{apply_tx_at, ChainState};
use noid_chain::Mempool;
use noid_poseidon2b::primitives::{Address, Digest};
use noid_tx::native_assets::launch::LaunchSide;
use noid_tx::native_assets::market::{MarketDefinition, PoolState, NOID_ASSET};
use noid_tx::native_assets::nft_market::queue::{FundedPurchaseRef, ListingBatch};
use noid_tx::native_assets::nft_market::settlement::ListingSettlement;
use noid_tx::native_assets::nft_market::{
    ListingDefinition, ListingState, NftRoundContext, PurchaseRequest,
    NFT_MAX_SETTLEMENT_FEE_PER_REQUEST,
};
use noid_tx::native_assets::trade::sequencing::{
    MarketStart, SequencingError, SequencingPlan, TradeBoundary,
};
use noid_tx::native_assets::trade::{TradeIntent, TradePolicy};
use noid_tx::native_assets::transition::NativeInputClaim;
use noid_tx::native_assets::NftItem;
use noid_tx::native_assets::{native_creation_id, AssetId};
use noid_tx::{
    output_bitmap_bit, validate_paged_spend, Transaction, TxBody, TxInput, TxOutput, TxPage,
    PAGED_SPEND_END_BIT, PAGED_SPEND_START_BIT, TX_INPUTS, TX_OUTPUTS,
};

// ---------------------------------------------------------------------------
// Tiny deterministic RNG (xorshift64*) so every run is reproducible.
// ---------------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn digest(&mut self) -> Digest {
        let mut d = [0u8; 32];
        for chunk in d.chunks_mut(8) {
            chunk.copy_from_slice(&self.next_u64().to_le_bytes()[..chunk.len()]);
        }
        d
    }
    fn address(&mut self) -> Address {
        Address(self.digest())
    }
}

fn header(line: &str) {
    println!("\n=== {line} ===");
}

fn check(cond: bool, what: &str) {
    assert!(cond, "ASSERTION FAILED: {what}");
    println!("  ok: {what}");
}

// ---------------------------------------------------------------------------
// Shared fixtures.
// ---------------------------------------------------------------------------

const ANCHOR: Digest = [0x31; 32];

fn user_pages(anchor: Digest, fee: u64, seed: u8) -> Vec<TxPage> {
    let mut inputs = [TxInput::dummy(); TX_INPUTS];
    inputs[0] = TxInput {
        slot_index: u32::from(seed) * 2 + 1,
        amount: 1_000_000 + fee,
        creation_id: u64::from(seed) + 1,
    };
    let mut outputs = [TxOutput::dummy(); TX_OUTPUTS];
    outputs[0] = TxOutput {
        slot_index: u32::from(seed) * 2 + 2,
        amount: 1_000_000,
        owner: Address([seed; 32]),
    };
    vec![TxPage::new(TxBody {
        epoch_anchor: anchor,
        fee,
        input_owner: Address([0xA5; 32]),
        inputs,
        outputs,
        validity_bitmap: 1 | output_bitmap_bit(0) | PAGED_SPEND_START_BIT | PAGED_SPEND_END_BIT,
        is_coinbase: false,
    })
    .unwrap()]
}

fn txid_of(pages: &[TxPage]) -> Digest {
    validate_paged_spend(pages).unwrap().logical_txid.0
}

// A structurally valid Buy trade intent funded by one plain UTXO at
// (slot_index, creation_id) owned by `owner`. This is the exact shape the
// production `funding_priority` ranks.
fn buy_intent(owner: Address, slot_index: u32, creation_id: u64, amount: u64) -> TradeIntent {
    TradeIntent {
        policy: TradePolicy {
            chain: [0x43; 32],
            epoch_anchor: [0x45; 32],
            owner,
            asset: AssetId([0x51; 32]),
            recipient: Address([0x52; 32]),
            side: LaunchSide::Buy,
            gross_input: 1_000_000,
            minimum_output: 1,
            fee_limit: 1,
            valid_from: 1,
            valid_until: 1_000,
        },
        funding: vec![NativeInputClaim {
            input: TxInput {
                slot_index,
                amount,
                creation_id,
            },
            root: owner,
            opening: None,
        }],
    }
}

fn market_start() -> MarketStart {
    let definition = MarketDefinition {
        genesis: [0x71; 32],
        base: NOID_ASSET,
        quote: AssetId([0x51; 32]),
        profile: 1,
    };
    definition.validate().unwrap();
    MarketStart::Market {
        definition,
        state: PoolState::bootstrap_permanent(definition, 1_000_000_000, 1_000_000).unwrap(),
    }
}

fn boundary(parent_id: Digest) -> TradeBoundary {
    TradeBoundary {
        chain: [0x43; 32],
        epoch_anchor: [0x45; 32],
        parent_id,
        height: 5,
    }
}

fn main() {
    println!("sequencing audit — parano1d @ 52938433a30b185ab007afaa15166a632cf2e82e");
    println!("build.rs verified the pinned ordering facts (fee keys, template sort, miner");
    println!("refresh guards, financial fee pinning, TRORD_21/NMPRIOR1 priority domains).");

    let mut rng = Rng::new(0x5EED);

    // =========================================================================
    header("B1 CONTROL — mempool selection is descending fee_rate, ascending txid");
    // =========================================================================
    {
        let mut pool = Mempool::new(16);
        let low = user_pages(ANCHOR, 1_000, 1);
        let high = user_pages(ANCHOR, 100_000, 2);
        let high_id = txid_of(&high);
        let low_id = txid_of(&low);
        pool.admit(low, 0).unwrap();
        pool.admit(high, 0).unwrap();
        let selected = pool.select_for_block(BLOCK_MAX_USER_PAGES);
        check(selected.len() == 2, "both entries selected");
        check(
            selected[0].spend.logical_txid.0 == high_id,
            "higher fee first",
        );
        check(
            selected[1].spend.logical_txid.0 == low_id,
            "lower fee second",
        );

        // Equal fee, equal shape -> pure txid tie-break.
        let a = user_pages(ANCHOR, 9_000, 3);
        let b = user_pages(ANCHOR, 9_000, 4);
        let a_id = txid_of(&a);
        let b_id = txid_of(&b);
        let mut tie = Mempool::new(8);
        tie.admit(if a_id > b_id { a.clone() } else { b.clone() }, 0)
            .unwrap();
        tie.admit(if a_id > b_id { b.clone() } else { a.clone() }, 0)
            .unwrap();
        let order = tie.select_for_block(BLOCK_MAX_USER_PAGES);
        check(
            order[0].spend.logical_txid.0 == a_id.min(b_id),
            "equal fees order by ascending txid",
        );
    }

    // =========================================================================
    header("B2 CONTROL — template fee order; consensus application is order-free");
    // =========================================================================
    {
        // Real chain state and parent header, mirroring the production
        // template tests.
        let owner = Address([0xA5; 32]);
        let mut state = ChainState::with_log_slots(8);
        for (slot, amount) in [(3u32, 1_009_000u64), (7, 1_040_000)] {
            state
                .state
                .set_slot(
                    slot,
                    noid_chain::SlotValue::with_owner_fields(amount, 42, owner.as_fields()),
                )
                .unwrap();
        }
        state.active_slot_count = 2;
        state.alloc_counter = 42;
        state.circulating_supply_micronoid = 2_049_000;
        let mut parent = genesis_header();
        parent.log_slots = 8;
        parent.state_root = state.state_root();
        parent.active_slot_count = 2;
        parent.alloc_counter = 42;
        let anchor = block_id(&parent);
        let miner = Address([9; 32]);
        let ts = parent.timestamp + 1;

        let low = {
            let mut pages = user_pages(anchor, 9_000, 1);
            pages[0].body.inputs[0] = TxInput {
                slot_index: 3,
                amount: 1_009_000,
                creation_id: 42,
            };
            pages
        };
        let high = {
            let mut pages = user_pages(anchor, 40_000, 4);
            pages[0].body.inputs[0] = TxInput {
                slot_index: 7,
                amount: 1_040_000,
                creation_id: 42,
            };
            pages
        };
        let low_id = txid_of(&low);
        let high_id = txid_of(&high);
        let users = [
            Transaction::new(low[0].body.clone()),
            Transaction::new(high[0].body.clone()),
        ];
        let template =
            build_block_template(&parent, &state, &[2], users.to_vec(), miner, ts, [0xff; 32])
                .unwrap();
        let block = template.clone().into_block(0);
        let user_idx: Vec<usize> = block
            .transactions
            .iter()
            .enumerate()
            .filter(|(_, t)| !t.body.is_coinbase)
            .map(|(i, _)| i)
            .collect();
        check(user_idx.len() == 2, "template carries two user groups");
        // Fee order in the produced template: high fee before low fee.
        check(
            block.transactions[user_idx[0]].body.fee == 40_000,
            "template places the 40,000-fee group before the 9,000-fee group",
        );
        let _ = (low_id, high_id);

        // Apply the user groups in both orders. Creation IDs are assigned
        // sequentially in application order, so the two orders reach
        // DIFFERENT state roots: in-block position determines the
        // creation_ids (future priority entropy) of every output.
        let mut forward = state.clone();
        for tx in &block.transactions {
            apply_tx_at(&mut forward, &tx.body, 1).unwrap();
        }
        let mut reversed = state.clone();
        for tx in block.transactions.iter().filter(|t| t.body.is_coinbase) {
            apply_tx_at(&mut reversed, &tx.body, 1).unwrap();
        }
        for &i in user_idx.iter().rev() {
            apply_tx_at(&mut reversed, &block.transactions[i].body, 1).unwrap();
        }
        check(
            forward.state_root() == template.state_root,
            "template order reproduces the template's own state root",
        );
        check(
            forward.state_root() != reversed.state_root(),
            "reversed order yields a different state root: creation ids are position-derived",
        );

        // A permuted block is a *different but constructible* block: recompute
        // its header roots for the permuted body order and the alternative
        // ordering is fully determined. Nothing in validate_block_consensus
        // inspects tx order (verified against the pinned source by build.rs),
        // so which ordering ships is purely the builder's choice.
        let mut permuted = block.clone();
        let (a, b) = (user_idx[0], user_idx[1]);
        permuted.transactions.swap(a, b);
        let original_root = noid_chain::block::compute_tx_root(&block.transactions);
        let permuted_tx_root = noid_chain::block::compute_tx_root(&permuted.transactions);
        check(
            original_root == template.tx_root,
            "compute_tx_root reproduces the template's root",
        );
        check(
            permuted_tx_root != template.tx_root,
            "permuted body order commits to a different tx root (order is consensus-tracked, not consensus-constrained)",
        );
    }

    // =========================================================================
    header("C  FINDING 1 — trade priority is a UTXO-count-weighted hash lottery");
    // =========================================================================
    // C1: production funding_priority over random parents.
    {
        const K: usize = 8; // attacker UTXOs
        const M: usize = 4; // victim UTXOs
        const TRIALS: usize = 2_000;
        let mut attacker_wins = 0usize;
        for _ in 0..TRIALS {
            let parent = rng.digest();
            let attacker: Vec<_> = (0..K)
                .map(|i| {
                    buy_intent(
                        Address([0xB0; 32]),
                        1_000 + i as u32,
                        100 + i as u64,
                        1_000_002,
                    )
                })
                .collect();
            let victims: Vec<_> = (0..M)
                .map(|i| {
                    buy_intent(
                        Address([0xC0; 32]),
                        2_000 + i as u32,
                        200 + i as u64,
                        1_000_002,
                    )
                })
                .collect();
            let best_attacker = attacker
                .iter()
                .map(|i| i.funding_priority(parent).unwrap())
                .min()
                .unwrap();
            let best_victim = victims
                .iter()
                .map(|i| i.funding_priority(parent).unwrap())
                .min()
                .unwrap();
            if best_attacker < best_victim {
                attacker_wins += 1;
            }
        }
        let rate = attacker_wins as f64 / TRIALS as f64;
        let expected = K as f64 / (K + M) as f64;
        println!(
            "  attacker tickets={K} victims={M} trials={TRIALS}: win rate {rate:.3} (K/(K+M)={expected:.3})"
        );
        check(
            (rate - expected).abs() < 0.03,
            "priority win rate matches the K/(K+M) exchangeable-lottery model",
        );

        // Priority ignores value: the minimum viable anchor equals a whale at
        // the same slot/creation.
        let dust = buy_intent(Address([0xB0; 32]), 77, 7, 1_000_002);
        let whale = buy_intent(Address([0xB0; 32]), 77, 7, 1_000_000_000_000);
        let parent = rng.digest();
        check(
            dust.funding_priority(parent).unwrap() == whale.funding_priority(parent).unwrap(),
            "priority is count-weighted, not value-weighted (dust = whale)",
        );

        // C2: the per-market batch cap truncates by this same priority order.
        let intents_64: Vec<_> = (0..64)
            .map(|i| buy_intent(Address([0xB0; 32]), 1_000 + i, 100 + i as u64, 1_000_002))
            .collect();
        let plan = SequencingPlan::execute(market_start(), boundary(rng.digest()), &intents_64);
        check(
            plan.is_ok(),
            "64 intents fill the batch cap exactly (MAX_SEQUENTIAL_TRADES)",
        );
        let intents_65: Vec<_> = (0..65)
            .map(|i| buy_intent(Address([0xB0; 32]), 2_000 + i, 300 + i as u64, 1_000_002))
            .collect();
        let over = SequencingPlan::execute(market_start(), boundary(rng.digest()), &intents_65);
        check(
            matches!(over, Err(SequencingError::Bounds)),
            "65 intents exceed the cap — capacity capture truncates by priority",
        );
        // Duplicate funding slot (same UTXO anchored twice) is rejected.
        let dup_a = buy_intent(Address([0xB0; 32]), 5_000, 900, 1_000_002);
        let dup_b = buy_intent(Address([0xC0; 32]), 5_000, 901, 1_000_002);
        let dup = SequencingPlan::execute(market_start(), boundary(rng.digest()), &[dup_a, dup_b]);
        check(
            matches!(dup, Err(SequencingError::DuplicateFunding)),
            "one UTXO cannot buy two draws in one batch (duplicate funding rejected)",
        );

        // C3: within one batch, execution order = funding-priority order.
        let intents: Vec<_> = (0..16)
            .map(|i| buy_intent(Address([0xB0; 32]), 30_000 + i, 5_000 + i as u64, 1_000_002))
            .collect();
        let parent = rng.digest();
        let priorities: Vec<_> = intents
            .iter()
            .map(|i| i.funding_priority(parent).unwrap())
            .collect();
        let plan = SequencingPlan::execute(market_start(), boundary(parent), &intents).unwrap();
        let mut expected_order: Vec<usize> = (0..16).collect();
        expected_order.sort_by_key(|&i| priorities[i]);
        let actual: Vec<usize> = plan.fills().iter().map(|f| f.source_index).collect();
        check(
            actual == expected_order,
            "fills execute in ascending funding-priority order (all-Buy batch)",
        );

        // The authorization-id key order is the canonical ENCODING only:
        // feed the same policies in reversed encoding order and the plan
        // still executes in digest order (source indexes remap, fills do not).
        let reversed: Vec<_> = intents.iter().rev().cloned().collect();
        let plan_r = SequencingPlan::execute(market_start(), boundary(parent), &reversed).unwrap();
        let actual_r: Vec<usize> = plan_r.fills().iter().map(|f| f.source_index).collect();
        let remapped: Vec<usize> = actual_r
            .iter()
            .map(|&i| 15 - i) // position i in the reversed slice is intent 15-i
            .collect();
        check(
            remapped == expected_order,
            "reversed authorization-key encoding still executes in funding-priority order",
        );

        // C4: ticket price — one persistent UTXO via the real fee model at
        // genesis occupancy (1 input -> 2 outputs, net +1 slot).
        let breakdown = fee_breakdown(1, 2, 0, LOG_SLOTS_GENESIS);
        println!(
            "  ticket price (1->2 outputs at 0 occupancy): base={} io={} state_growth={} total={} micronoid",
            breakdown.base, breakdown.io, breakdown.state_growth, breakdown.required_total
        );
        check(
            breakdown.required_total == breakdown.base + breakdown.io + breakdown.state_growth,
            "fee components sum to the required total",
        );
        println!(
            "  => a K/(K+M) perpetual priority share costs ~{} x K micronoid one-time (dust, persistent)",
            breakdown.required_total
        );
    }

    // =========================================================================
    header("D  FINDING 2 — NFT fixed-price races are a payer-address hash lottery");
    // =========================================================================
    {
        let trials = 400usize;
        let k = 12usize; // attacker pre-funded addresses (one draw each)
        let m = 4usize; // victim addresses
        let mut wins = 0usize;
        let mut rng_local = Rng::new(0xBEEF);
        for t in 0..trials {
            let height = 10_000u64 + t as u64;
            let context = NftRoundContext::new(height, rng_local.digest(), 10_000).unwrap();
            let item = NftItem {
                collection: noid_tx::native_assets::CollectionId([0xC1; 32]),
                owner: Address([9; 32]),
                metadata: [0x9D; 32],
                serial: 1,
                unlock_height: 0,
            };
            let definition = ListingDefinition::from_item(
                item,
                native_creation_id(7).unwrap(),
                200_000,
                height + 20,
                height,
            )
            .unwrap();
            let listing = ListingState::initial(definition).unwrap();
            let round = listing.round_id(context).unwrap();

            let mut requests = Vec::new();
            let mut best: Option<(Digest, Address)> = None;
            for i in 0..k {
                // Attacker draws K addresses offline; only the best is funded.
                let mut bytes = [0xB0 + i as u8; 32];
                bytes[0] ^= t as u8;
                bytes[1] ^= (t >> 8) as u8;
                let payer = Address(bytes);
                let priority = PurchaseRequest {
                    listing: listing.listing,
                    round,
                    payer,
                    recipient: Address([0x52; 32]),
                    budget: 200_000,
                    registered_height: height,
                    profile: 1,
                }
                .payer_priority();
                if best.map_or(true, |(d, _)| priority < d) {
                    best = Some((priority, payer));
                }
            }
            let (_, winner_address) = best.unwrap();
            requests.push(FundedPurchaseRef {
                slot_index: 900,
                creation_id: native_creation_id(900).unwrap(),
                request: PurchaseRequest {
                    listing: listing.listing,
                    round,
                    payer: winner_address,
                    recipient: Address([0x52; 32]),
                    budget: 200_000,
                    registered_height: height,
                    profile: 1,
                },
            });
            for i in 0..m {
                requests.push(FundedPurchaseRef {
                    slot_index: 901 + i as u32,
                    creation_id: native_creation_id(901 + i as u64).unwrap(),
                    request: PurchaseRequest {
                        listing: listing.listing,
                        round,
                        payer: Address([(0xC0 + i as u8); 32]),
                        recipient: Address([0x52; 32]),
                        budget: 200_000,
                        registered_height: height,
                        profile: 1,
                    },
                });
            }
            let batch = ListingBatch::register(definition, listing, context, &requests).unwrap();
            let settlement = ListingSettlement::calculate(
                &batch,
                height + 1,
                NFT_MAX_SETTLEMENT_FEE_PER_REQUEST,
            )
            .unwrap();
            let winner_request = requests
                .iter()
                .find(|r| r.key() == settlement.winner())
                .expect("settlement winner matches a registered request");
            wins += usize::from(winner_request.request.payer == winner_address);
        }
        let rate = wins as f64 / trials as f64;
        let expected = k as f64 / (k + m) as f64;
        println!(
            "  attacker draws={k} (submits 1) victims={m}: win rate {rate:.3} vs K/(K+M)={expected:.3}",
        );
        check(
            (rate - expected).abs() < 0.08,
            "ground payer wins at the predicted offline-grind rate",
        );

        // D2: the round is height-bound, so a grind only covers one candidate
        // registration height; the builder's inclusion-height choice forces
        // re-grinding per height.
        let height = 20_000u64;
        let context_h = NftRoundContext::new(height, [0x77; 32], 10_000).unwrap();
        let context_h1 = NftRoundContext::new(height + 1, [0x77; 32], 10_000).unwrap();
        let item = NftItem {
            collection: noid_tx::native_assets::CollectionId([0xC1; 32]),
            owner: Address([9; 32]),
            metadata: [0x9D; 32],
            serial: 2,
            unlock_height: 0,
        };
        let definition = ListingDefinition::from_item(
            item,
            native_creation_id(7).unwrap(),
            200_000,
            height + 20,
            height,
        )
        .unwrap();
        let listing = ListingState::initial(definition).unwrap();
        let round_h = listing.round_id(context_h).unwrap();
        let round_h1 = listing.round_id(context_h1).unwrap();
        check(
            round_h != round_h1,
            "round id changes with the registration height",
        );
        let stale = FundedPurchaseRef {
            slot_index: 900,
            creation_id: native_creation_id(900).unwrap(),
            request: PurchaseRequest {
                listing: listing.listing,
                round: round_h,
                payer: Address([0xB0; 32]),
                recipient: Address([0x52; 32]),
                budget: 200_000,
                registered_height: height,
                profile: 1,
            },
        };
        let ok = ListingBatch::register(definition, listing, context_h, &[stale]);
        check(
            ok.is_ok(),
            "request bound to round(h) registers at height h",
        );
        let wrong = ListingBatch::register(definition, listing, context_h1, &[stale]);
        check(
            wrong.is_err(),
            "the same request is rejected at height h+1 (height-specific grind)",
        );
    }

    // =========================================================================
    header("E  FINDING 3 — financial natives: pinned fees, txid tie-break, free grind");
    // =========================================================================
    {
        // Two equal-fee equal-shape groups: selection reduces to txid order.
        let a = user_pages(ANCHOR, 9_000, 3);
        let b = user_pages(ANCHOR, 9_000, 4);
        let a_id = txid_of(&a);
        let b_id = txid_of(&b);
        check(a_id != b_id, "equal-shape groups have distinct txids");
        let mut pool = Mempool::new(8);
        pool.admit(a, 0).unwrap();
        pool.admit(b, 0).unwrap();
        let selected = pool.select_for_block(1);
        check(
            selected.len() == 1 && selected[0].spend.logical_txid.0 == a_id.min(b_id),
            "one-slot truncation keeps the smaller txid (fees pinned equal)",
        );

        // A txid grind: pick the output owner until the txid beats the
        // victim's. Each try is one body hash, offline, free.
        let mut rng_local = Rng::new(0x600D);
        let mut attempts = Vec::new();
        for _ in 0..200 {
            let victim_owner = Address(rng_local.digest());
            let victim_pages = {
                let mut p = user_pages(ANCHOR, 9_000, 5);
                p[0].body.outputs[0].owner = victim_owner;
                p
            };
            let victim_id = txid_of(&victim_pages);
            let mut tries = 0u32;
            loop {
                tries += 1;
                let attacker_owner = Address(rng_local.digest());
                let attacker_pages = {
                    let mut p = user_pages(ANCHOR, 9_000, 6);
                    p[0].body.outputs[0].owner = attacker_owner;
                    p
                };
                if txid_of(&attacker_pages) < victim_id {
                    break;
                }
            }
            attempts.push(tries);
        }
        attempts.sort_unstable();
        let median = attempts[attempts.len() / 2];
        println!(
            "  txid grind vs a fixed victim: median {median} body hashes (mean {:.2}) over 200 races",
            attempts.iter().map(|a| *a as f64).sum::<f64>() / attempts.len() as f64
        );
        check(
            median <= 4,
            "beating a victim's txid costs a handful of offline hashes",
        );
        // The financial fee is pinned to the exact parent-derived minimum
        // (verified in the pinned source by build.rs), so no fee lever exists:
        // equal-shape financial natives cannot outbid each other.
        let minimum = fee_breakdown(1, 2, 0, LOG_SLOTS_GENESIS).required_total;
        println!(
            "  financial natives must pay exactly the parent-derived minimum ({minimum} here); no tipping"
        );
    }

    // =========================================================================
    header("F  FINDING 4 — miner inclusion freeze (verified statically)");
    // =========================================================================
    {
        // build.rs asserted both pinned guards: heartbeat and TxAdmitted
        // refreshes fire only while user_pages == 0. Consequence printed for
        // the record; no runtime component below the miner's event loop.
        println!("  pinned guard: `_ = &mut heartbeat, if user_pages == 0 =>`");
        println!(
            "  pinned guard: `refresh = next_mining_mempool_refresh(...), if user_pages == 0 =>`"
        );
        println!("  => once a template holds any user page, later admissions (any fee)");
        println!("     cannot enter the block being mined; inclusion is frozen at build time.");
    }

    println!("\nALL ASSERTIONS PASSED");
}
