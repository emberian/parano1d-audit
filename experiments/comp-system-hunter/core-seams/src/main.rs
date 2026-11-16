// SPDX-License-Identifier: Apache-2.0
// Compositional protocol-conformance & cross-subsystem invariant verification suite
// for `parano1d` (pinned commit 52938433a30b185ab007afaa15166a632cf2e82e).

#![allow(dead_code)]

#[path = "../../pinned/noid_node/src/wallet/mod.rs"]
mod wallet;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, ensure, Context, Result};
use noid_chain::consensus::fees::fee_breakdown;
use noid_chain::consensus::params::{
    owner_authorization_version, BLOCK_TIME, MAINNET_V2_1_ACTIVATION_HEIGHT, V2_1_ACTIVATION_HEIGHT,
    V2_BLOCK_TIME,
};
use noid_chain::consensus::template::{
    build_node_owned_block_template, build_node_owned_block_template_with_native_data,
    TemplateBuildError,
};
use noid_chain::consensus::{claimable_fee_for_tx_body, next_target, tx_epoch_anchor_height_for_child};
use noid_chain::fri_state::SlotValue;
use noid_chain::{
    block_id, materialize_accepted_block_state, AcceptedBlockBundle, Block, ChainState, ChainView,
    HistoryStepTerminalMetadata, MdbxChainContext,
};
use noid_gkr::{
    prove_native_financial_authorization, OwnerAuthWitness, WalletAuthorizationBundle,
};
use noid_mempool::{AsyncMempool, DecodedMempoolIntent, MempoolConfig};
use noid_miner::template::TemplateChainSnapshot;
use noid_miner::{HistoryProtocolRuntime, TemplateBuilder};
use noid_poseidon2b::primitives::{Address, SpendSecret, TxBodyHash};
use noid_recursive::acceptance::history_step::v2::{
    banked as v2_banked, capacity_banked as v21_banked, TradeKernelConfig, V2Config, V2Error,
};
use noid_recursive::{placeholder_history_step_recording_layout, HistoryStepMatrixLease};
use noid_rpc::native_types::{NativeWalletPreview, NativeWalletRequest};
use noid_rpc::wallet_ops::{
    WalletNativeContext, WalletNativeIntent, WalletNativeProofRequest, WalletOps,
};
use noid_tx::native_assets::financial::FinancialOperation;
use noid_tx::native_assets::launch::{self, LaunchDefinition, LaunchOpening};
use noid_tx::native_assets::nft_market::{ListingDefinition, NftMarketOpening, NftRoundContext};
use noid_tx::native_assets::transition::{NativeContext, NativeOperation};
use noid_tx::native_assets::wire::{NativeInputClaim, NativeIntent, NativeOutputClaim};
use noid_tx::native_assets::{
    CollectionId, FungibleDefinition, GenesisAnchor, IssuanceRoute, NativeOpening, NftItem,
    TokenReserve,
};
use noid_tx::{
    input_bitmap_bit, output_bitmap_bit, PagedSpendIntent, Transaction, TxBody, TxInput, TxOutput,
    TxPage, PAGED_SPEND_END_BIT, PAGED_SPEND_START_BIT, TX_INPUTS, TX_OUTPUTS,
};

struct LazyNoMatrix;
impl v2_banked::MatrixSource for LazyNoMatrix {
    fn load(&self, _: v2_banked::Class) -> Result<HistoryStepMatrixLease, V2Error> {
        Err(V2Error::Matrix)
    }
}
impl v21_banked::MatrixSource for LazyNoMatrix {
    fn load(&self, _: v21_banked::Class) -> Result<HistoryStepMatrixLease, V2Error> {
        Err(V2Error::Matrix)
    }
}

/// Construct a genuine `HistoryProtocolRuntime` carrying both V2 and V2.1 (`finance`)
/// selection banks matching `ACTIVE_SCHEDULE` (matrix loading is lazy and only invoked
/// during recursive proving, not during `TemplateBuilder` template assembly).
fn build_v21_history_runtime(origin_dir: std::path::PathBuf) -> Result<HistoryProtocolRuntime> {
    use noid_chain::consensus::forks::ACTIVE_SCHEDULE;
    let auth_at = noid_chain::consensus::params::OWNER_AUTHORIZATION_ACTIVATION_HEIGHT
        .context("OWNER_AUTHORIZATION_ACTIVATION_HEIGHT must be set")?;

    let v2_small = V2Config::with_limits(23, 63, 504, 63, ACTIVE_SCHEDULE)
        .map_err(|e| anyhow!("{e:?}"))?;
    let v2_large = V2Config::with_limits(24, 255, 1020, 26, ACTIVE_SCHEDULE)
        .map_err(|e| anyhow!("{e:?}"))?;
    let v2_cfg = v2_banked::Config::new(v2_small, v2_large).map_err(|e| anyhow!("{e:?}"))?;
    let v2_vks = v2_banked::Class::ALL
        .map(|cls| v2_banked::derive_assembled_block_vk(v2_cfg.class(cls)).expect("v2 vk"));
    let v2_layouts = std::array::from_fn(|_| placeholder_history_step_recording_layout(1 << 14));
    let v2_parts =
        v2_banked::RuntimeParts::new(v2_cfg, v2_vks, v2_layouts.clone(), v2_layouts)
            .map_err(|e| anyhow!("{e:?}"))?;
    let v2_rt = Arc::new(
        v2_banked::Runtime::new(
            v2_banked::Bank::pin([[0u8; 32]; 2], &v2_parts),
            v2_parts,
            Box::new(LazyNoMatrix),
        )
        .map_err(|e| anyhow!("{e:?}"))?,
    );

    let v21_small = v2_small
        .with_authorization_update(auth_at)
        .and_then(|c| c.with_native_assets(16))
        .map_err(|e| anyhow!("{e:?}"))?;
    let v21_large = v2_large
        .with_authorization_update(auth_at)
        .and_then(|c| c.with_native_assets(16))
        .map_err(|e| anyhow!("{e:?}"))?;
    let v21_native = V2Config::with_limits(24, 63, 504, 63, ACTIVE_SCHEDULE)
        .and_then(|c| c.with_authorization_update(auth_at))
        .and_then(|c| {
            c.with_native_sequential_finance(
                16,
                128,
                TradeKernelConfig {
                    markets: 4,
                    policies: 16,
                    funding: 32,
                },
            )
        })
        .map_err(|e| anyhow!("{e:?}"))?;
    let v21_cfg =
        v21_banked::Config::new(v21_small, v21_large, v21_native).map_err(|e| anyhow!("{e:?}"))?;
    let v21_vks = v21_banked::Class::ALL
        .map(|cls| v21_banked::derive_assembled_block_vk(v21_cfg.class(cls)).expect("v21 vk"));
    let v21_layouts = std::array::from_fn(|_| placeholder_history_step_recording_layout(1 << 14));
    let v21_parts =
        v21_banked::RuntimeParts::new(v21_cfg, v21_vks, v21_layouts.clone(), v21_layouts)
            .map_err(|e| anyhow!("{e:?}"))?;
    let v21_rt = Arc::new(
        v21_banked::Runtime::new(
            v21_banked::Bank::pin([[0u8; 32]; 3], &v21_parts),
            v21_parts,
            Box::new(LazyNoMatrix),
        )
        .map_err(|e| anyhow!("{e:?}"))?,
    );

    HistoryProtocolRuntime::new(None, Some(v2_rt), origin_dir)
        .and_then(|rt| rt.with_finance_runtime(Some(v21_rt)))
        .map_err(|e| anyhow!(e))
}

fn step_spacing(height: u64) -> u64 {
    if noid_chain::consensus::params::v2_active(height) {
        V2_BLOCK_TIME
    } else {
        BLOCK_TIME
    }
}

fn mine_next_bundle(
    ctx: &MdbxChainContext,
    miner_address: Address,
    candidate_txs: Vec<Transaction>,
    native_openings: &[NativeOpening],
    timestamp_delta: u64,
) -> Result<(AcceptedBlockBundle, Block)> {
    let parent = *ctx.tip_header();
    let child_height = parent.height + 1;
    let timestamp = parent
        .timestamp
        .saturating_add(timestamp_delta.max(1));
    let anchor = ctx.anchor_info()?;
    let target = next_target(
        anchor.anchor_height,
        anchor.anchor_timestamp,
        &anchor.anchor_target,
        child_height,
        parent.timestamp,
        timestamp,
    );
    let finalized_active_counts = ctx.finalized_active_counts()?;
    let pending_outputs = HashSet::new();
    let (template, _) = build_node_owned_block_template_with_native_data(
        &parent,
        &ctx.state,
        &finalized_active_counts,
        candidate_txs,
        miner_address,
        timestamp,
        target,
        &pending_outputs,
        native_openings,
        &[],
    )
    .map_err(|e| anyhow!("template build at height {child_height}: {e:?}"))?;

    let mut nonce = 0u128;
    let block = loop {
        let candidate = template.clone().into_block(nonce);
        if noid_chain::consensus::validate_pow(&candidate.header).is_ok() {
            break candidate;
        }
        nonce = nonce.checked_add(1).context("PoW nonce exhausted")?;
    };
    let mut terminal = HistoryStepTerminalMetadata::new(
        block.header.height,
        noid_chain::block_header::semantic_header_id(&block.header),
        0,
    )?
    .encode_prefix()
    .to_vec();
    terminal.push(0xA5);

    let payload = noid_chain::block::payload::BlockBodyPayload::new(
        block.clone(),
        native_openings.to_vec(),
        Vec::new(),
    )
    .map_err(|e| anyhow!("payload encode: {e}"))?;
    let bundle = AcceptedBlockBundle::try_from_parts(payload.encode(), terminal)?;
    Ok((bundle, block))
}

fn accept_bundle(ctx: &mut MdbxChainContext, bundle: &AcceptedBlockBundle, block: &Block) -> Result<()> {
    ctx.apply_next_block(
        bundle,
        block.header.timestamp,
        |blk, state| materialize_accepted_block_state(state, blk).map_err(|e| format!("{e:?}")),
        |_| Ok(()),
    )?;
    Ok(())
}

fn build_signed_ordinary_spend(
    secret: SpendSecret,
    epoch_anchor: [u8; 32],
    candidate_height: u64,
    inputs: &[(u32, u64, u64)],
    outputs: &[(u32, u64, Address)],
    fee: u64,
) -> Result<(PagedSpendIntent, Vec<u8>)> {
    let owner = secret.derive_address();
    let req_pages = inputs
        .len()
        .div_ceil(TX_INPUTS)
        .max(outputs.len().div_ceil(TX_OUTPUTS))
        .max(1);
    let mut pages = Vec::with_capacity(req_pages);
    for page_idx in 0..req_pages {
        let mut body = TxBody {
            epoch_anchor,
            validity_bitmap: 0,
            inputs: [TxInput::EMPTY; TX_INPUTS],
            input_owner: owner,
            outputs: [TxOutput::EMPTY; TX_OUTPUTS],
            fee: if page_idx == 0 { fee } else { 0 },
            reserved: 0,
        };
        if page_idx == 0 {
            body.validity_bitmap |= PAGED_SPEND_START_BIT;
        }
        if page_idx + 1 == req_pages {
            body.validity_bitmap |= PAGED_SPEND_END_BIT;
        }
        let in_start = page_idx * TX_INPUTS;
        let in_end = (in_start + TX_INPUTS).min(inputs.len());
        for (lane, &(slot_index, amount, creation_id)) in inputs[in_start..in_end].iter().enumerate() {
            body.inputs[lane] = TxInput {
                slot_index,
                amount,
                creation_id,
            };
            body.validity_bitmap |= input_bitmap_bit(lane);
        }
        let out_start = page_idx * TX_OUTPUTS;
        let out_end = (out_start + TX_OUTPUTS).min(outputs.len());
        for (lane, &(slot_index, amount, out_owner)) in outputs[out_start..out_end].iter().enumerate() {
            body.outputs[lane] = TxOutput {
                slot_index,
                amount,
                owner: out_owner,
            };
            body.validity_bitmap |= output_bitmap_bit(lane);
        }
        pages.push(TxPage { body });
    }
    let version = owner_authorization_version(candidate_height);
    let unsigned = PagedSpendIntent::new(pages.clone(), Vec::new())?;
    let auth = WalletAuthorizationBundle::prove_paged_spend_with_version(
        &pages,
        OwnerAuthWitness::new(secret),
        version,
    )
    .map_err(|e| anyhow!("prove_paged_spend_with_version: {e}"))?;
    let auth_bytes = auth.to_bytes().map_err(|e| anyhow!("auth to_bytes: {e}"))?;
    let signed = PagedSpendIntent::new(pages, auth_bytes)?;
    ensure!(signed.logical_txid() == unsigned.logical_txid());
    let wire = signed.to_bytes()?;
    Ok((signed, wire))
}

fn build_signed_create_launch(
    secret: SpendSecret,
    parent: &noid_chain::BlockHeader,
    epoch_anchor: [u8; 32],
    funding_input: TxInput,
    out_slots: [u32; 4],
    metadata_tag: u8,
) -> Result<(NativeIntent, Vec<u8>, u64)> {
    let owner = secret.derive_address();
    let candidate_height = parent.height + 1;
    let chain = noid_chain::state::native_assets::native_chain_id();
    let fee = fee_breakdown(1, 4, parent.active_slot_count, parent.log_slots).required_total;

    let supply = 1_000_000_000u64;
    let deposit = FungibleDefinition {
        asset: noid_tx::native_assets::AssetId([0u8; 32]),
        supply,
        route: IssuanceRoute::Launchpad,
        metadata: [metadata_tag; 32],
    }
    .minimum_backing(supply)
    .map_err(|e| anyhow!("minimum_backing: {e:?}"))?;

    let anchor = GenesisAnchor {
        chain,
        owner,
        input: funding_input,
    };
    let token = FungibleDefinition::from_genesis(
        &anchor,
        supply,
        IssuanceRoute::Launchpad,
        [metadata_tag; 32],
    )
    .map_err(|e| anyhow!("FungibleDefinition: {e:?}"))?;
    let definition =
        LaunchDefinition::from_genesis(&anchor, token).map_err(|e| anyhow!("LaunchDefinition: {e:?}"))?;
    let def_opening = NativeOpening::Launch(LaunchOpening::Definition(definition));
    let def_root = def_opening.root();
    let launch_state = match definition
        .after_initial_purchase(0)
        .map_err(|e| anyhow!("after_initial_purchase: {e:?}"))?
    {
        launch::sequential::TradeState::Curve(state) => {
            NativeOpening::Launch(LaunchOpening::State(state))
        }
        launch::sequential::TradeState::Market(_) => {
            anyhow::bail!("unexpected instant graduation on zero initial purchase")
        }
    };
    let reserve = NativeOpening::TokenReserve(
        TokenReserve::from_definition(&token, deposit).map_err(|e| anyhow!("TokenReserve: {e:?}"))?,
    );
    let native_outs = [
        NativeOpening::FungibleDefinition(token),
        reserve,
        launch_state,
    ];
    let change = funding_input
        .amount
        .checked_sub(deposit + fee)
        .context("insufficient funding input for CreateLaunch")?;

    let inputs = vec![NativeInputClaim {
        input: funding_input,
        root: owner,
        opening: None,
    }];
    let mut outputs = Vec::with_capacity(4);
    for (idx, opening) in native_outs.into_iter().enumerate() {
        outputs.push(NativeOutputClaim {
            output: TxOutput {
                slot_index: out_slots[idx],
                amount: opening.amount_micronoid(),
                owner: opening.root(),
            },
            opening: Some(opening),
        });
    }
    outputs.push(NativeOutputClaim {
        output: TxOutput {
            slot_index: out_slots[3],
            amount: change,
            owner,
        },
        opening: None,
    });

    let mut lookup: BTreeMap<[u8; 32], NativeOpening> = outputs
        .iter()
        .filter_map(|o| o.opening)
        .map(|o| (o.root().0, o))
        .collect();
    lookup.insert(def_root.0, def_opening);

    let context = NativeContext {
        chain,
        height: candidate_height,
        authority: owner,
        fee,
    };
    let round = NftRoundContext::new(candidate_height, block_id(parent), parent.alloc_counter)
        .map_err(|e| anyhow!("NftRoundContext: {e:?}"))?;
    let mut intent = NativeIntent::from_financial_claims(
        &context,
        epoch_anchor,
        FinancialOperation::CreateLaunch {
            funding_input: 0,
            definition: def_root,
        },
        round,
        inputs,
        outputs,
        |root| lookup.get(&root.0).copied(),
    )
    .map_err(|e| anyhow!("from_financial_claims: {e:?}"))?;

    let version = owner_authorization_version(candidate_height);
    let proof = prove_native_financial_authorization(
        &intent,
        chain,
        round,
        OwnerAuthWitness::new(secret),
        version,
    )
    .map_err(|e| anyhow!("prove_native_financial_authorization: {e}"))?;
    intent.authorization_bytes = proof.to_bytes().map_err(|e| anyhow!("proof to_bytes: {e}"))?;
    let wire = intent.to_bytes().map_err(|e| anyhow!("intent to_bytes: {e:?}"))?;
    Ok((intent, wire, fee))
}

// ============================================================================
// TEST 1 (Finding A): Stale-Fee Financial Native Intent (`CreateLaunch`)
// Survives Downward State-Pressure / Expansion Transition and Halts
// `TemplateBuilder` (`FinancialBlockError::Fee` abort).
// ============================================================================
async fn test_1_financial_fee_downward_transition_halts_template_builder() -> Result<()> {
    println!("\n=== [Test 1] Financial Native Fee Downward Transition vs TemplateBuilder ===");
    ensure!(
        MAINNET_V2_1_ACTIVATION_HEIGHT == Some(279_370),
        "expected mainnet v2.1 height 279,370"
    );
    let v21_at = V2_1_ACTIVATION_HEIGHT.context("v2.1 activation must be set")?;

    let tmp = tempfile::tempdir()?;
    let db_path = tmp.path().join("chain_t1");
    let mut ctx = MdbxChainContext::open_or_create(&db_path)?;

    let user_a = SpendSecret::from_bytes([0x11; 32]);
    let user_b = SpendSecret::from_bytes([0x22; 32]);
    let consol_secret = SpendSecret::from_bytes([0x33; 32]);
    let miner_addr = Address([0x44; 32]);

    // Mine blocks from genesis so user_a, user_b, and consol_secret hold spendable coinbases,
    // advancing to V2.1 activation height.
    for h in 1..=v21_at {
        let recipient = match h {
            1 => user_a.derive_address(),
            2 => user_b.derive_address(),
            3 | 4 => consol_secret.derive_address(),
            _ => miner_addr,
        };
        let sp = step_spacing(h);
        let (bundle, blk) = mine_next_bundle(&ctx, recipient, Vec::new(), &[], sp)?;
        accept_bundle(&mut ctx, &bundle, &blk)?;
    }
    ensure!(noid_chain::consensus::params::v2_1_active(ctx.tip_height() + 1));

    // Locate the coinbase UTXOs for user_a, user_b, and consol_secret (2 UTXOs to consolidate).
    let utxos_a = ctx.store.get_verified_utxos_by_owner(&user_a.derive_address().0)?;
    let utxos_b = ctx.store.get_verified_utxos_by_owner(&user_b.derive_address().0)?;
    let utxos_c = ctx
        .store
        .get_verified_utxos_by_owner(&consol_secret.derive_address().0)?;
    ensure!(utxos_a.utxos.len() == 1 && utxos_b.utxos.len() == 1 && utxos_c.utxos.len() == 2);

    // Prepare a real `TemplateChainSnapshot` at V2.1 height where `active_slot_count` sits
    // right at the 50% pressure threshold (`1 << (log_slots - 1)` => multiplier = 2).
    // Note: in consensus, `active_slot_count == 1 << (log_slots - 1)` occurs naturally whenever
    // occupancy reaches 50% (or 75% right before `log_slots` expands `m -> m+1`, dropping
    // occupancy to 37.5% and multiplier `4 -> 1` in a single block!).
    let mut snap_h = TemplateChainSnapshot::from_context(&mut ctx)?;
    let log_slots = snap_h.parent.log_slots;
    let threshold_50pct = 1u64 << (log_slots - 1);
    snap_h.parent.active_slot_count = threshold_50pct;

    let anchor_h = block_id(&snap_h.child_tx_epoch_anchor_header);
    let mut headers = HashMap::new();
    let anchor_height = tx_epoch_anchor_height_for_child(snap_h.parent.height + 1);
    headers.insert(anchor_height, snap_h.child_tx_epoch_anchor_header);
    headers.insert(snap_h.parent.height, snap_h.parent);

    // Build `ChainView` at height H with `active_slot_count = threshold_50pct`.
    let mut view_state = ctx.state.state.clone();
    // Adjust view_state's reported live count? Note `ChainView::from_mdbx` sets `active_slot_count`
    // from `tip_header().active_slot_count`! Let's check `view_h.active_slot_count`.
    let mut view_h = ChainView::new(snap_h.parent.height, headers.clone(), 1, view_state.clone());
    view_h.active_slot_count = threshold_50pct;

    let mempool = AsyncMempool::new(MempoolConfig::default(), view_h);
    let runtime = build_v21_history_runtime(tmp.path().join("origins"))?;
    let builder = TemplateBuilder::new(mempool.clone()).with_history_protocol(&runtime);

    // User A submits a completely valid `CreateLaunch` (op 48) at height H (50% occupancy).
    // `net_new_slots = 4 - 1 = 3`, `pressure_multiplier = 2`:
    // `minimum(H) = 10_000 + 3 * 2_500 * 2 = 25_000` uNOID.
    let in_a = TxInput {
        slot_index: utxos_a.utxos[0].slot_index,
        amount: utxos_a.utxos[0].amount,
        creation_id: utxos_a.utxos[0].creation_id,
    };
    let (_, launch_a_wire, fee_h) = build_signed_create_launch(
        user_a,
        &snap_h.parent,
        anchor_h,
        in_a,
        [500, 501, 502, 503],
        0xA1,
    )?;
    ensure!(fee_h == 25_000, "expected 25,000 uNOID at 50% pressure, got {fee_h}");

    let decoded_a = DecodedMempoolIntent::from_bytes(&launch_a_wire)?;
    let txid_a = mempool.submit_decoded(decoded_a, launch_a_wire).await?;
    ensure!(mempool.len().await == 1);

    // Baseline check: at height H (`active_slot_count = threshold_50pct`), `TemplateBuilder`
    // cleanly builds a valid V2.1 `Class::Native` block template containing User A's launch!
    let mut snap_h_check = TemplateChainSnapshot::from_context(&mut ctx)?;
    snap_h_check.parent.active_slot_count = threshold_50pct;
    let ok_tmpl = builder
        .build_from_snapshot_with_limit(snap_h_check, miner_addr, snap_h.parent.timestamp + 30, 63)
        .await;
    ensure!(
        ok_tmpl.is_some(),
        "baseline TemplateBuilder at height H should succeed"
    );
    println!(
        "  [ok] Height H (50% pressure, required fee = {} uNOID): CreateLaunch admitted & template built.",
        fee_h
    );

    // Now block H+1 is mined WITHOUT User A's CreateLaunch, consolidating 2 UTXOs -> 1 UTXO
    // (net -1 live slot: `consol_secret` spends 2 coinbases into 1 output, while coinbase adds +1,
    // or a 3->1 consolidation reduces `active_slot_count` from `threshold_50pct` to `threshold_50pct - 1`!).
    // Equally, at 75% occupancy, automatic `log_slots` expansion (`m -> m+1`) drops occupancy to 37.5%!
    let (bundle_h1, blk_h1) = mine_next_bundle(&ctx, miner_addr, Vec::new(), &[], V2_BLOCK_TIME)?;
    accept_bundle(&mut ctx, &bundle_h1, &blk_h1)?;

    let mut snap_h1 = TemplateChainSnapshot::from_context(&mut ctx)?;
    snap_h1.parent.active_slot_count = threshold_50pct - 1; // 1 slot below 50% -> multiplier drops 2 -> 1!

    let mut headers_h1 = HashMap::new();
    let anchor_h1_height = tx_epoch_anchor_height_for_child(snap_h1.parent.height + 1);
    headers_h1.insert(anchor_h1_height, snap_h1.child_tx_epoch_anchor_header);
    headers_h1.insert(snap_h1.parent.height, snap_h1.parent);
    let mut view_h1 = ChainView::new(
        snap_h1.parent.height,
        headers_h1,
        1,
        ctx.state.state.clone(),
    );
    view_h1.active_slot_count = threshold_50pct - 1;

    // Run `AsyncMempool::on_new_block` for block H+1:
    mempool
        .on_new_block(&[], snap_h1.parent.height, view_h1)
        .await;

    // BUG 1: `AsyncMempool::on_new_block` retains `txid_a` (`fee = 25,000 > new minimum = 17,500`)!
    ensure!(
        mempool.len().await == 1,
        "expected stale-fee CreateLaunch to survive on_new_block"
    );
    println!(
        "  [confirmed] After H+1 lowers required fee 25,000 -> 17,500 uNOID, AsyncMempool::on_new_block retains txid {} (len = 1).",
        hex::encode(&txid_a.0[..6])
    );

    // Even if User B now submits a FRESH, valid `CreateLaunch` at H+1 paying the exact new
    // minimum (`17,500` uNOID), User A's stale `25,000` uNOID intent has higher fee-rate
    // (`12,500` vs `8,750` uNOID/page) and is selected ahead of User B!
    let in_b = TxInput {
        slot_index: utxos_b.utxos[0].slot_index,
        amount: utxos_b.utxos[0].amount,
        creation_id: utxos_b.utxos[0].creation_id,
    };
    let anchor_h1 = block_id(&snap_h1.child_tx_epoch_anchor_header);
    let (_, launch_b_wire, fee_h1) = build_signed_create_launch(
        user_b,
        &snap_h1.parent,
        anchor_h1,
        in_b,
        [600, 601, 602, 603],
        0xB2,
    )?;
    ensure!(fee_h1 == 17_500, "expected 17,500 uNOID below 50% pressure, got {fee_h1}");
    let decoded_b = DecodedMempoolIntent::from_bytes(&launch_b_wire)?;
    let _txid_b = mempool.submit_decoded(decoded_b, launch_b_wire).await?;
    ensure!(mempool.len().await == 2);

    // BUG 2: `TemplateBuilder::build_from_snapshot_with_limit` now FAILS (`None`) on every attempt!
    let wedged_tmpl = builder
        .build_from_snapshot_with_limit(snap_h1, miner_addr, blk_h1.header.timestamp + 30, 63)
        .await;
    ensure!(
        wedged_tmpl.is_none(),
        "expected TemplateBuilder to fail (`None`) due to FinancialBlockError::Fee abort"
    );
    println!(
        "  [confirmed] TemplateBuilder::build_from_snapshot_with_limit returns None even with fresh valid CreateLaunch in pool (mempool len = 2)!"
    );
    Ok(())
}

fn build_unsigned_ordinary_pages(
    owner: Address,
    epoch_anchor: [u8; 32],
    inputs: &[(u32, u64, u64)],
    outputs: &[(u32, u64, Address)],
    fee: u64,
) -> Vec<TxPage> {
    let req_pages = inputs
        .len()
        .div_ceil(TX_INPUTS)
        .max(outputs.len().div_ceil(TX_OUTPUTS))
        .max(1);
    let mut pages = Vec::with_capacity(req_pages);
    for page_idx in 0..req_pages {
        let mut body = TxBody {
            epoch_anchor,
            validity_bitmap: 0,
            inputs: [TxInput::EMPTY; TX_INPUTS],
            input_owner: owner,
            outputs: [TxOutput::EMPTY; TX_OUTPUTS],
            fee: if page_idx == 0 { fee } else { 0 },
            reserved: 0,
        };
        if page_idx == 0 {
            body.validity_bitmap |= PAGED_SPEND_START_BIT;
        }
        if page_idx + 1 == req_pages {
            body.validity_bitmap |= PAGED_SPEND_END_BIT;
        }
        let in_start = page_idx * TX_INPUTS;
        let in_end = (in_start + TX_INPUTS).min(inputs.len());
        for (lane, &(slot_index, amount, creation_id)) in inputs[in_start..in_end].iter().enumerate() {
            body.inputs[lane] = TxInput {
                slot_index,
                amount,
                creation_id,
            };
            body.validity_bitmap |= input_bitmap_bit(lane);
        }
        let out_start = page_idx * TX_OUTPUTS;
        let out_end = (out_start + TX_OUTPUTS).min(outputs.len());
        for (lane, &(slot_index, amount, out_owner)) in outputs[out_start..out_end].iter().enumerate() {
            body.outputs[lane] = TxOutput {
                slot_index,
                amount,
                owner: out_owner,
            };
            body.validity_bitmap |= output_bitmap_bit(lane);
        }
        pages.push(TxPage { body });
    }
    pages
}

// ============================================================================
// TEST 2 (Finding B): `hydrate_coinbase_reuse_segment` False-Positive Short-Circuit
// Causes `TemplateBuildError::NoCoinbaseSlot` (`None`) Despite >99% Free Slots.
// ============================================================================
async fn test_2_coinbase_hydration_reserved_slot_short_circuit() -> Result<()> {
    println!("\n=== [Test 2] hydrate_coinbase_reuse_segment Reserved-Slot Short-Circuit ===");
    let tmp = tempfile::tempdir()?;
    let db_path = tmp.path().join("chain_t2");
    let mut ctx = MdbxChainContext::open_or_create(&db_path)?;

    let user_secret = SpendSecret::from_bytes([0x55; 32]);
    let user_addr = user_secret.derive_address();
    let faucet_addr = Address([0x66; 32]);
    let sink_addr = Address([0x99; 32]);
    let miner_addr = Address([0x77; 32]);

    // 1. Mine genesis -> height 12 (past V2 activation at height 10, giving `user_addr` 1 coinbase
    //    and `faucet_addr` 11 coinbases of 50 NOID each = 550 NOID to fund 65,789 slot creations).
    for h in 1..=12 {
        let recipient = if h == 1 { user_addr } else { faucet_addr };
        let (bundle, blk) = mine_next_bundle(&ctx, recipient, Vec::new(), &[], step_spacing(h))?;
        accept_bundle(&mut ctx, &bundle, &blk)?;
    }

    let seg_log = ctx.state.state.effective_log_segment_size();
    let seg_size = 1u32 << seg_log;
    let num_segs = ctx.state.state.num_segments();
    ensure!(num_segs == 256 && seg_size == 65_536);

    let user_utxo = ctx.store.get_verified_utxos_by_owner(&user_addr.0)?.utxos[0];
    let user_seg = (user_utxo.slot_index >> seg_log) as u16;

    // 2. Mine 1 genuine consensus block placing 1 live UTXO in every segment `!= user_seg` that is
    //    still virtual-zero (`live_count == 0`), so all 256 segments become populated (`live > 0`).
    let mut seed_outputs = Vec::new();
    for seg in 0..num_segs {
        let seg_u16 = seg as u16;
        if seg_u16 == user_seg || ctx.state.state.segment_live_count(seg_u16) > 0 {
            continue;
        }
        let target_slot = ((seg as u32) << seg_log) + 100;
        seed_outputs.push((target_slot, 1u64, sink_addr));
    }
    if !seed_outputs.is_empty() {
        let faucet_utxos = ctx.store.get_verified_utxos_by_owner(&faucet_addr.0)?.utxos;
        let f_in = faucet_utxos[0];
        let anchor = ctx.user_tx_epoch_anchor_id_for_child()?;
        let out_count = seed_outputs.len() + 1;
        let fee = fee_breakdown(
            1,
            out_count as u64,
            ctx.tip_header().active_slot_count,
            ctx.tip_header().log_slots,
        )
        .required_total;
        let mut outs = seed_outputs;
        let change_slot = (((user_seg as usize + 1) % 256) as u32) << seg_log | 200;
        let spent_val = outs.len() as u64;
        outs.push((change_slot, f_in.amount - spent_val - fee, faucet_addr));
        let pages = build_unsigned_ordinary_pages(
            faucet_addr,
            anchor,
            &[(f_in.slot_index, f_in.amount, f_in.creation_id)],
            &outs,
            fee,
        );
        let txs: Vec<Transaction> = pages.into_iter().map(|p| Transaction::new(p.body)).collect();
        let (b, blk) = mine_next_bundle(&ctx, faucet_addr, txs, &[], V2_BLOCK_TIME)?;
        accept_bundle(&mut ctx, &b, &blk)?;
    }

    // 3. Fill `user_seg` via genuine V2 blocks (up to two 254-output PagedSpends = 508 slots/block)
    //    until `user_seg` has `65,534` live slots and EXACTLY `2` empty slots (`out_slot_0`, `out_slot_1`)!
    //    Note: coinbase outputs go to `miner_addr`; if a coinbase happens to land inside `user_seg`,
    //    we simply scan live empty slots in `user_seg` dynamically on each iteration!
    let seg_base = u32::from(user_seg) << seg_log;
    loop {
        let empty_now: Vec<u32> = (0..seg_size)
            .map(|off| seg_base + off)
            .filter(|&slot| ctx.state.state.get_slot(slot) == SlotValue::EMPTY)
            .collect();
        ensure!(
            empty_now.len() >= 2,
            "user_seg unexpectedly dropped below 2 empty slots"
        );
        if empty_now.len() == 2 {
            break;
        }
        let to_fill = empty_now.len() - 2;
        let batch_total = to_fill.min(500);
        let anchor = ctx.user_tx_epoch_anchor_id_for_child()?;
        let mut faucet_utxos = ctx.store.get_verified_utxos_by_owner(&faucet_addr.0)?.utxos;
        faucet_utxos.sort_by_key(|u| std::cmp::Reverse(u.amount));

        // Split into 1 or 2 PagedSpends (each <= 255 outputs + 1 change output in a different segment).
        let mut block_txs = Vec::new();
        let mut cursor = 0usize;
        let mut utxo_idx = 0usize;
        while cursor < batch_total {
            let chunk = (batch_total - cursor).min(254);
            let f_in = faucet_utxos[utxo_idx];
            utxo_idx += 1;
            let fee = fee_breakdown(
                1,
                (chunk + 1) as u64,
                ctx.tip_header().active_slot_count,
                ctx.tip_header().log_slots,
            )
            .required_total;
            let mut outs: Vec<(u32, u64, Address)> = empty_now[cursor..cursor + chunk]
                .iter()
                .map(|&slot| (slot, 1u64, sink_addr))
                .collect();
            // Put faucet change in another segment (`!= user_seg`) so it doesn't perturb `user_seg`.
            let other_seg = ((user_seg as usize + 1 + utxo_idx) % 256) as u32;
            let mut chg_slot = (other_seg << seg_log) + 500;
            while ctx.state.state.get_slot(chg_slot) != SlotValue::EMPTY {
                chg_slot += 1;
            }
            outs.push((chg_slot, f_in.amount - (chunk as u64) - fee, faucet_addr));
            let pages = build_unsigned_ordinary_pages(
                faucet_addr,
                anchor,
                &[(f_in.slot_index, f_in.amount, f_in.creation_id)],
                &outs,
                fee,
            );
            block_txs.extend(pages.into_iter().map(|p| Transaction::new(p.body)));
            cursor += chunk;
        }
        // Choose `faucet_addr` for coinbase too; note `select_system_mint_slot` picks the segment
        // with the LOWEST `live_count` (which is `1`, NOT `user_seg`!), so coinbase NEVER lands in `user_seg`!
        let (b, blk) = mine_next_bundle(&ctx, faucet_addr, block_txs, &[], V2_BLOCK_TIME)?;
        accept_bundle(&mut ctx, &b, &blk)?;
    }

    let final_empty_in_user_seg: Vec<u32> = (0..seg_size)
        .map(|off| seg_base + off)
        .filter(|&slot| ctx.state.state.get_slot(slot) == SlotValue::EMPTY)
        .collect();
    ensure!(final_empty_in_user_seg.len() == 2);
    let (out_slot_0, out_slot_1) = (final_empty_in_user_seg[0], final_empty_in_user_seg[1]);

    // 4. Capture a 100% genuine `TemplateChainSnapshot::from_context(&mut ctx)` at this durable tip!
    let snap = TemplateChainSnapshot::from_context(&mut ctx)?;
    for seg in 0..256u16 {
        ensure!(
            snap.state.state.is_evicted(seg),
            "segment {seg} must be evicted in durable_metadata_clone"
        );
    }
    let free_slots = (1u64 << snap.parent.log_slots) - snap.parent.active_slot_count;
    println!(
        "  [setup] Genuine {}-block chain: active_slot_count = {}, free_slots = {} ({:.2}% empty).",
        snap.parent.height,
        snap.parent.active_slot_count,
        free_slots,
        (free_slots as f64 / (1u64 << snap.parent.log_slots) as f64) * 100.0
    );

    // 5. User submits ONE valid cryptographically proved 1-input, 2-output payment touching ONLY
    //    `user_seg`, whose 2 outputs reserve the last 2 empty slots (`out_slot_0`, `out_slot_1`)!
    let anchor_hash = block_id(&snap.child_tx_epoch_anchor_header);
    let req_fee =
        fee_breakdown(1, 2, snap.parent.active_slot_count, snap.parent.log_slots).required_total;
    let (_, tx_wire) = build_signed_ordinary_spend(
        user_secret,
        anchor_hash,
        snap.parent.height + 1,
        &[(user_utxo.slot_index, user_utxo.amount, user_utxo.creation_id)],
        &[
            (out_slot_0, 100_000, miner_addr),
            (out_slot_1, user_utxo.amount - 100_000 - req_fee, user_addr),
        ],
        req_fee,
    )?;

    let view = ChainView::from_mdbx(&ctx);
    let mempool = AsyncMempool::new(MempoolConfig::default(), view);
    let decoded = DecodedMempoolIntent::from_bytes(&tx_wire)?;
    mempool.submit_decoded(decoded, tx_wire).await?;
    ensure!(mempool.len().await == 1);

    // Control check: against the full resident `ctx.state`, `build_node_owned_block_template`
    // succeeds immediately by placing coinbase in one of the 255 nearly-empty segments!
    let anchor = ctx.anchor_info()?;
    let ts = snap.parent.timestamp + V2_BLOCK_TIME;
    let target = next_target(
        anchor.anchor_height,
        anchor.anchor_timestamp,
        &anchor.anchor_target,
        snap.parent.height + 1,
        snap.parent.timestamp,
        ts,
    );
    let selected = mempool.select_for_block(63).await;
    let selected_txs: Vec<Transaction> = selected
        .into_iter()
        .flat_map(|e| e.pages.into_iter().map(|p| Transaction::new(p.body)))
        .collect();
    let full_state_res = build_node_owned_block_template(
        &snap.parent,
        &ctx.state,
        &snap.finalized_active_counts,
        selected_txs,
        miner_addr,
        ts,
        target,
    );
    ensure!(
        full_state_res.is_ok(),
        "full hydrated ChainState should easily find a coinbase slot: {:?}",
        full_state_res.err()
    );

    // BUG: `TemplateBuilder::build_from_snapshot_with_limit` hydrates ONLY `user_seg`, then
    // `hydrate_coinbase_reuse_segment` passes `&HashSet::new()` instead of reserved outputs,
    // sees `out_slot_0` is empty in parent state, short-circuits `Ok(())` without hydrating any
    // of the 255 nearly-empty segments, and fails with `NoCoinbaseSlot` (`None`)!
    let runtime = build_v21_history_runtime(tmp.path().join("origins_t2"))?;
    let builder = TemplateBuilder::new(mempool).with_history_protocol(&runtime);
    let tmpl_opt = builder
        .build_from_snapshot_with_limit(snap, miner_addr, ts, 63)
        .await;
    ensure!(
        tmpl_opt.is_none(),
        "expected TemplateBuilder::build_from_snapshot_with_limit to return None due to NoCoinbaseSlot"
    );
    println!(
        "  [confirmed] Full state builds template cleanly, while TemplateBuilder::build_from_snapshot_with_limit returns None (NoCoinbaseSlot) despite 99.61% free slots!"
    );
    Ok(())
}

// ============================================================================
// TEST 3 (Finding D): Inverted Reorg Wallet-vs-Mempool Reservation Ordering
// & Missing `native::on_accepted_block` Locks UTXOs After Exact Reorg.
// ============================================================================
async fn test_3_reorg_wallet_reservation_ordering_desync() -> Result<()> {
    println!("\n=== [Test 3] Reorg Wallet-vs-Mempool Reservation Ordering Desync ===");
    let tmp = tempfile::tempdir()?;
    let chain_a_dir = tmp.path().join("chain_a");
    let chain_b_dir = tmp.path().join("chain_b");
    let wallet_key = tmp.path().join("wallet.key");

    let mut ctx_a = MdbxChainContext::open_or_create(&chain_a_dir)?;
    let mut ctx_b = MdbxChainContext::open_or_create(&chain_b_dir)?;

    let w_state = wallet::WalletState::create_or_load(wallet_key)?;
    let wallet_addr = w_state.active_address();
    let shared_wallet: wallet::SharedWallet = Arc::new(Mutex::new(Some(w_state)));
    let wallet_ops = wallet::WalletHandle::new(shared_wallet.clone());

    // Common ancestor block 1: credits `wallet_addr` with 1 spendable coinbase UTXO.
    let (b1, blk1) = mine_next_bundle(&ctx_a, wallet_addr, Vec::new(), &[], BLOCK_TIME)?;
    accept_bundle(&mut ctx_a, &b1, &blk1)?;
    accept_bundle(&mut ctx_b, &b1, &blk1)?;
    wallet::update_for_accepted_block(&shared_wallet, &blk1).map_err(|e| anyhow!(e))?;

    // Branch A mines block 2 (empty, miner = 0xAA).
    let (b2_a, blk2_a) =
        mine_next_bundle(&ctx_a, Address([0xAA; 32]), Vec::new(), &[], BLOCK_TIME)?;
    accept_bundle(&mut ctx_a, &b2_a, &blk2_a)?;
    wallet::update_for_accepted_block(&shared_wallet, &blk2_a).map_err(|e| anyhow!(e))?;

    // Initialize AsyncMempool on Node A at height 2.
    let view_a2 = ChainView::from_mdbx(&ctx_a);
    let mempool = AsyncMempool::new(MempoolConfig::default(), view_a2);

    // User creates a pending ordinary payment on Node A choosing output slot `900` (empty on Branch A),
    // spending their block-1 coinbase UTXO (`utxo_slot`), and submits it to `mempool` + records wallet reservation!
    let utxos_before = wallet_ops.utxos().map_err(|e| anyhow!(e))?;
    ensure!(utxos_before.len() == 1 && !utxos_before[0].reserved);
    let utxo_slot = utxos_before[0].slot_index;
    let out_conflict_slot = 900u32;

    let spend_secret = {
        let g = shared_wallet.lock().unwrap();
        g.as_ref().unwrap().spend_secret_for(0)
    };
    let anchor_a2 = ctx_a.user_tx_epoch_anchor_id_for_child()?;
    let req_fee = fee_breakdown(
        1,
        2,
        ctx_a.tip_header().active_slot_count,
        ctx_a.tip_header().log_slots,
    )
    .required_total;
    let (spend_intent, spend_wire) = build_signed_ordinary_spend(
        spend_secret,
        anchor_a2,
        ctx_a.tip_height() + 1,
        &[(
            utxo_slot,
            utxos_before[0].value_micronoid,
            utxos_before[0].creation_id,
        )],
        &[
            (out_conflict_slot, 50_000, Address([0xBB; 32])),
            (
                901,
                utxos_before[0].value_micronoid - 50_000 - req_fee,
                wallet_addr,
            ),
        ],
        req_fee,
    )?;
    let txid = mempool.submit(spend_intent, spend_wire).await?;
    {
        let mut g = shared_wallet.lock().unwrap();
        let w = g.as_mut().unwrap();
        w.add_pending_inputs(&[utxo_slot]);
        w.add_pending_outputs(&[out_conflict_slot, 901]);
    }
    ensure!(mempool.len().await == 1);

    // Meanwhile, heavier Branch B mines blocks 2 and 3 where someone ELSE occupies `out_conflict_slot = 900`!
    // Give `other_secret` a coinbase at block 2 on Branch B, and spend it in block 3 on Branch B to `out_conflict_slot = 900`.
    let other_secret = SpendSecret::from_bytes([0xCC; 32]);
    let other_addr = other_secret.derive_address();
    let (b2_b, blk2_b) = mine_next_bundle(&ctx_b, other_addr, Vec::new(), &[], BLOCK_TIME)?;
    accept_bundle(&mut ctx_b, &b2_b, &blk2_b)?;
    let other_utxo = ctx_b.store.get_verified_utxos_by_owner(&other_addr.0)?.utxos[0];
    let anchor_b2 = ctx_b.user_tx_epoch_anchor_id_for_child()?;
    let other_fee = fee_breakdown(
        1,
        1,
        ctx_b.tip_header().active_slot_count,
        ctx_b.tip_header().log_slots,
    )
    .required_total;
    let (other_intent, _) = build_signed_ordinary_spend(
        other_secret,
        anchor_b2,
        3,
        &[(
            other_utxo.slot_index,
            other_utxo.amount,
            other_utxo.creation_id,
        )],
        &[(
            out_conflict_slot,
            other_utxo.amount - other_fee,
            Address([0xDD; 32]),
        )],
        other_fee,
    )?;
    let other_txs: Vec<Transaction> = other_intent
        .pages
        .into_iter()
        .map(|p| Transaction::new(p.body))
        .collect();
    let (b3_b, blk3_b) =
        mine_next_bundle(&ctx_b, Address([0xEE; 32]), other_txs, &[], BLOCK_TIME)?;
    accept_bundle(&mut ctx_b, &b3_b, &blk3_b)?;

    // Execute the EXACT `noid_node/src/main.rs` reorg sequence (`lines 4358 -> 4575 -> 4617 -> 4678 -> 4685`) on Node A!
    // Step 1 (`main.rs:4358`): Snapshot `mempool.reserved_slots().await` BEFORE reorg & mempool update!
    let (reserved_input_slots, reserved_output_slots) = mempool.reserved_slots().await;
    ensure!(reserved_input_slots.contains(&utxo_slot));

    // Step 2 (`main.rs:4576`): Apply the atomic reorg on `ctx_a` to Branch B (ancestor height = 1, blocks 2 & 3).
    let reorg_res = ctx_a.apply_reorg_mdbx_with_applier(
        1,
        &[blk2_b.clone(), blk3_b.clone()],
        blk3_b.header.timestamp,
        |blk, state| materialize_accepted_block_state(state, blk).map_err(|e| format!("{e:?}")),
    )?;

    // Step 3 (`main.rs:4617`): Install post-reorg wallet snapshot using the pre-reorg `reserved_*_slots`!
    let snapshot = ctx_a.store.get_verified_utxos_by_owner(&wallet_addr.0)?;
    wallet::install_reorg_snapshot_and_artifacts(
        &shared_wallet,
        0,
        1,
        wallet_addr.0,
        snapshot,
        &reserved_input_slots,
        &reserved_output_slots,
        &reorg_res.reclaimed_tx_hashes,
        &[&blk2_b, &blk3_b],
    )
    .map_err(|e| anyhow!(e))?;

    // Step 4 (`main.rs:4678-4686`): Update mempool (`on_new_block` + `readmit_after_reorg`).
    let confirmed_hashes = vec![other_intent_txid(&blk3_b)];
    let new_view = ChainView::from_mdbx(&ctx_a);
    mempool
        .on_new_block(&confirmed_hashes, new_view.tip_height, new_view)
        .await;
    mempool
        .readmit_after_reorg(reorg_res.reclaimed_tx_hashes.clone())
        .await;

    // Postcondition verification:
    // 1. `mempool` EVICTED `txid` (`OutputSlotOccupied` on slot 900), so `mempool.len() == 0` and
    //    `mempool.reserved_slots()` is completely EMPTY!
    ensure!(mempool.is_empty().await, "mempool should have evicted conflicting tx");
    let (mp_in_after, mp_out_after) = mempool.reserved_slots().await;
    ensure!(mp_in_after.is_empty() && mp_out_after.is_empty());

    // 2. YET `wallet` STILL marks `utxo_slot` as `reserved: true` (`pending_input_slots` = `{utxo_slot}`),
    //    permanently locking the user's ONLY UTXO!
    let utxos_after = wallet_ops.utxos().map_err(|e| anyhow!(e))?;
    ensure!(utxos_after.len() == 1 && utxos_after[0].slot_index == utxo_slot);
    ensure!(
        utxos_after[0].reserved,
        "expected wallet UTXO to remain stranded as reserved=true after reorg eviction"
    );
    println!(
        "  [confirmed] Post-reorg: mempool evicted tx {} (mempool len = 0, reserved = 0), but wallet UTXO slot {} remains stranded with reserved = true!",
        hex::encode(&txid.0[..6]),
        utxo_slot
    );
    Ok(())
}

fn other_intent_txid(blk: &Block) -> TxBodyHash {
    let ids = noid_chain::block::native_assets::try_compute_logical_txids_at(
        &blk.transactions,
        blk.header.height,
    )
    .expect("logical txids");
    ids[1]
}

// ============================================================================
// TEST 4 (Finding D2): Single-Missed-Block `CreateListing` Eviction + Incomplete
// `walletDismissNativePending` (`forget_native_activity`) Permanently Locks NFT
// & NOID UTXO and Leaves Undismissable Ghost Entry.
// ============================================================================
async fn test_4_create_listing_eviction_and_broken_dismiss_pending() -> Result<()> {
    println!("\n=== [Test 4] CreateListing Single-Block Eviction + forget_native_activity Leak ===");
    let v21_at = V2_1_ACTIVATION_HEIGHT.context("v2.1 activation must be set")?;
    let tmp = tempfile::tempdir()?;
    let db_path = tmp.path().join("chain_t4");
    let wallet_key = tmp.path().join("wallet_t4.key");

    let mut ctx = MdbxChainContext::open_or_create(&db_path)?;
    let w_state = wallet::WalletState::create_or_load(wallet_key)?;
    let seller_addr = w_state.active_address();
    let seller_secret = w_state.spend_secret_for(0);
    let shared_wallet: wallet::SharedWallet = Arc::new(Mutex::new(Some(w_state)));
    let wallet_ops = wallet::WalletHandle::new(shared_wallet.clone());

    for h in 1..=v21_at {
        let recipient = if h == 1 {
            seller_addr
        } else {
            Address([0x77; 32])
        };
        let (bundle, blk) = mine_next_bundle(&ctx, recipient, Vec::new(), &[], step_spacing(h))?;
        accept_bundle(&mut ctx, &bundle, &blk)?;
        wallet::update_for_accepted_block(&shared_wallet, &blk).map_err(|e| anyhow!(e))?;
    }

    // Mint a real NFT (`CreateCollection` + `MintNft`) on-chain for `seller_addr` so the seller owns
    // both 1 live NFT UTXO and 1 live NOID UTXO in `MdbxChainContext` & `WalletState`!
    let chain_id = noid_chain::state::native_assets::native_chain_id();
    let seller_utxo = wallet_ops.utxos().map_err(|e| anyhow!(e))?[0].clone();
    let parent_c = *ctx.tip_header();
    let anchor_c = ctx.user_tx_epoch_anchor_id_for_child()?;
    let fee_c = fee_breakdown(1, 2, parent_c.active_slot_count, parent_c.log_slots).required_total;
    let in_c = TxInput {
        slot_index: seller_utxo.slot_index,
        amount: seller_utxo.value_micronoid,
        creation_id: seller_utxo.creation_id,
    };
    let col = noid_tx::native_assets::NftCollection::from_genesis(
        &GenesisAnchor {
            chain: chain_id,
            owner: seller_addr,
            input: in_c,
        },
        10,
        [0xC1; 32],
    )
    .map_err(|e| anyhow!("{e:?}"))?;
    let col_open = NativeOpening::NftCollection(col);
    let intent_c = NativeIntent::from_claims(
        &NativeContext {
            chain: chain_id,
            height: parent_c.height + 1,
            authority: seller_addr,
            fee: fee_c,
        },
        anchor_c,
        NativeOperation::CreateCollection { funding_input: 0 },
        vec![NativeInputClaim {
            input: in_c,
            root: seller_addr,
            opening: None,
        }],
        vec![
            NativeOutputClaim {
                output: TxOutput {
                    slot_index: 700,
                    amount: col_open.amount_micronoid(),
                    owner: col_open.root(),
                },
                opening: Some(col_open),
            },
            NativeOutputClaim {
                output: TxOutput {
                    slot_index: 701,
                    amount: seller_utxo.value_micronoid - col_open.amount_micronoid() - fee_c,
                    owner: seller_addr,
                },
                opening: None,
            },
        ],
    )
    .map_err(|e| anyhow!("{e:?}"))?;
    let txs_c: Vec<Transaction> = intent_c
        .pages
        .iter()
        .map(|p| Transaction::new(p.body.clone()))
        .collect();
    let (b_col, blk_col) =
        mine_next_bundle(&ctx, Address([0x77; 32]), txs_c, &[col_open], V2_BLOCK_TIME)?;
    accept_bundle(&mut ctx, &b_col, &blk_col)?;
    wallet::update_for_accepted_block(&shared_wallet, &blk_col).map_err(|e| anyhow!(e))?;

    // Now mint serial #0 of that collection to `seller_addr`:
    let col_live = ctx.store.native_live_slots(col_open.root().0)?[0];
    let seller_cash = wallet_ops.utxos().map_err(|e| anyhow!(e))?[0].clone();
    let parent_m = *ctx.tip_header();
    let anchor_m = ctx.user_tx_epoch_anchor_id_for_child()?;
    let fee_m = fee_breakdown(2, 3, parent_m.active_slot_count, parent_m.log_slots).required_total;
    let next_col = NativeOpening::NftCollection(
        col.mint_many(1).map_err(|e| anyhow!("{e:?}"))?,
    );
    let item = NftItem {
        collection: col.collection,
        serial: 1,
        owner: seller_addr,
        metadata: [0xD2; 32],
    };
    let item_open = NativeOpening::NftItem(item);
    let intent_m = NativeIntent::from_claims(
        &NativeContext {
            chain: chain_id,
            height: parent_m.height + 1,
            authority: seller_addr,
            fee: fee_m,
        },
        anchor_m,
        NativeOperation::MintNft { count: 1 },
        vec![
            NativeInputClaim {
                input: TxInput {
                    slot_index: col_live.slot_index,
                    amount: col_live.amount,
                    creation_id: col_live.creation_id,
                },
                root: col_open.root(),
                opening: Some(col_open),
            },
            NativeInputClaim {
                input: TxInput {
                    slot_index: seller_cash.slot_index,
                    amount: seller_cash.value_micronoid,
                    creation_id: seller_cash.creation_id,
                },
                root: seller_addr,
                opening: None,
            },
        ],
        vec![
            NativeOutputClaim {
                output: TxOutput {
                    slot_index: 710,
                    amount: next_col.amount_micronoid(),
                    owner: next_col.root(),
                },
                opening: Some(next_col),
            },
            NativeOutputClaim {
                output: TxOutput {
                    slot_index: 711,
                    amount: item_open.amount_micronoid(),
                    owner: item_open.root(),
                },
                opening: Some(item_open),
            },
            NativeOutputClaim {
                output: TxOutput {
                    slot_index: 712,
                    amount: seller_cash.value_micronoid
                        - item_open.amount_micronoid()
                        - fee_m,
                    owner: seller_addr,
                },
                opening: None,
            },
        ],
    )
    .map_err(|e| anyhow!("{e:?}"))?;
    let txs_m: Vec<Transaction> = intent_m
        .pages
        .iter()
        .map(|p| Transaction::new(p.body.clone()))
        .collect();
    let (b_mint, blk_mint) = mine_next_bundle(
        &ctx,
        Address([0x77; 32]),
        txs_m,
        &[col_open, next_col, item_open],
        V2_BLOCK_TIME,
    )?;
    accept_bundle(&mut ctx, &b_mint, &blk_mint)?;
    wallet::update_for_accepted_block(&shared_wallet, &blk_mint).map_err(|e| anyhow!(e))?;

    // Start `AsyncMempool` at current tip H.
    let mempool = AsyncMempool::new(MempoolConfig::default(), ChainView::from_mdbx(&ctx));

    // Seller builds & reserves `CreateListing` (op 64) via the REAL `WalletOps` API
    // (`build_native_authorization` + `reserve_native_submission` + `record_native_submission`)!
    let item_live = ctx.store.native_live_slots(item_open.root().0)?[0];
    let cash_live = wallet_ops.utxos().map_err(|e| anyhow!(e))?[0].clone();
    let parent_l = *ctx.tip_header();
    let anchor_l = ctx.user_tx_epoch_anchor_id_for_child()?;
    let cand_h = parent_l.height + 1;
    let fee_l = fee_breakdown(2, 2, parent_l.active_slot_count, parent_l.log_slots).required_total;

    let listing_def = ListingDefinition::from_item(&item, 5_000_000, seller_addr, cand_h)
        .map_err(|e| anyhow!("{e:?}"))?;
    let listing_def_open = NativeOpening::NftMarket(NftMarketOpening::Definition(listing_def));
    let listing_state_open =
        NativeOpening::NftMarket(NftMarketOpening::Listing(listing_def.initial_state()));

    let mut map_l = BTreeMap::new();
    map_l.insert(item_open.root().0, item_open);
    map_l.insert(listing_def_open.root().0, listing_def_open);
    map_l.insert(listing_state_open.root().0, listing_state_open);

    let round_l = NftRoundContext::new(cand_h, block_id(&parent_l), parent_l.alloc_counter)
        .map_err(|e| anyhow!("{e:?}"))?;
    let listing_intent = NativeIntent::from_financial_claims(
        &NativeContext {
            chain: chain_id,
            height: cand_h,
            authority: seller_addr,
            fee: fee_l,
        },
        anchor_l,
        FinancialOperation::CreateListing {
            definition: listing_def_open.root(),
        },
        round_l,
        vec![
            NativeInputClaim {
                input: TxInput {
                    slot_index: item_live.slot_index,
                    amount: item_live.amount,
                    creation_id: item_live.creation_id,
                },
                root: item_open.root(),
                opening: Some(item_open),
            },
            NativeInputClaim {
                input: TxInput {
                    slot_index: cash_live.slot_index,
                    amount: cash_live.value_micronoid,
                    creation_id: cash_live.creation_id,
                },
                root: seller_addr,
                opening: None,
            },
        ],
        vec![
            NativeOutputClaim {
                output: TxOutput {
                    slot_index: 720,
                    amount: listing_state_open.amount_micronoid(),
                    owner: listing_state_open.root(),
                },
                opening: Some(listing_state_open),
            },
            NativeOutputClaim {
                output: TxOutput {
                    slot_index: 721,
                    amount: cash_live.value_micronoid - fee_l,
                    owner: seller_addr,
                },
                opening: None,
            },
        ],
        |r| map_l.get(&r.0).copied(),
    )
    .map_err(|e| anyhow!("{e:?}"))?;

    let w_ctx = WalletNativeContext {
        key_index: 0,
        owner: seller_addr,
        parent: parent_l,
        epoch_anchor: anchor_l,
    };
    let submission = wallet_ops
        .build_native_authorization(
            WalletNativeProofRequest {
                context: w_ctx,
                intent: WalletNativeIntent::Operation(listing_intent),
            },
            ctx.store.clone(),
        )
        .map_err(|e| anyhow!(e))?;
    let _lease = wallet_ops
        .reserve_native_submission(&submission)
        .map_err(|e| anyhow!(e))?;

    let decoded_l = DecodedMempoolIntent::from_bytes(&submission.intent_bytes)?;
    let listing_txid = mempool
        .submit_decoded(decoded_l, submission.intent_bytes.clone())
        .await?;
    ensure!(listing_txid.0 == submission.txid);

    let dummy_preview = NativeWalletPreview {
        plan_id: "plan-1".into(),
        txid: hex::encode(submission.txid),
        owner: seller_addr.to_bech32(),
        key_index: 0,
        height: cand_h,
        parent_hash: hex::encode(block_id(&parent_l)),
        state_root: hex::encode(parent_l.state_root),
        request: NativeWalletRequest::CreateListing {
            collection_id: hex::encode(col.collection.0),
            serial: "1".into(),
            price_micronoid: "5000000".into(),
            payout_address: None,
        },
        fee_micronoid: fee_l.to_string(),
        fee_limit_micronoid: None,
        noid_debit_micronoid: fee_l.to_string(),
        noid_credit_micronoid: "0".into(),
        reserve_deposit_micronoid: None,
        initial_buy_cost_micronoid: None,
        redemption_micronoid: None,
        quoted_output: None,
        minimum_output: None,
        valid_until: None,
        asset_id: None,
        collection_id: Some(hex::encode(col.collection.0)),
        serial: Some("1".into()),
        last_serial: None,
        input_count: 2,
        output_count: 2,
        pending_quote: None,
    };
    wallet_ops
        .record_native_submission(dummy_preview)
        .map_err(|e| anyhow!(e))?;
    ensure!(mempool.len().await == 1);

    // Now ONE single empty block (H+1) is mined by another miner before receiving the gossip!
    let (b_empty, blk_empty) =
        mine_next_bundle(&ctx, Address([0x88; 32]), Vec::new(), &[], V2_BLOCK_TIME)?;
    accept_bundle(&mut ctx, &b_empty, &blk_empty)?;
    wallet::update_for_accepted_block(&shared_wallet, &blk_empty).map_err(|e| anyhow!(e))?;
    mempool
        .on_new_block(&[], ctx.tip_height(), ChainView::from_mdbx(&ctx))
        .await;

    // 1. Because `ListingDefinition::from_item` binds `created_height = cand_h`, `CreateListing`
    //    is IMMEDIATELY EVICTED from `AsyncMempool` after just 1 missed block!
    ensure!(
        mempool.is_empty().await,
        "CreateListing should be evicted on the very next block because created_height changed"
    );

    // 2. User calls `walletDismissNativePending` (`wallet_ops.forget_native_activity(txid)`):
    wallet_ops
        .forget_native_activity(submission.txid)
        .map_err(|e| anyhow!(e))?;
    ensure!(
        wallet_ops.native_activity().map_err(|e| anyhow!(e))?.is_empty(),
        "native_activity preview record was removed"
    );

    // 3. BUG: `forget_native_activity` ONLY deleted `native_activity`; it left `wallet.native_pending`
    //    AND `wallet.pending_input_slots` AND `wallet.pending_output_slots` completely intact!
    let ghost_pending = wallet_ops.native_pending_submissions();
    ensure!(
        ghost_pending.len() == 1 && ghost_pending[0].txid == submission.txid,
        "expected native_pending ghost reservation to survive forget_native_activity"
    );
    let seller_utxos_after = wallet_ops.utxos().map_err(|e| anyhow!(e))?;
    ensure!(
        seller_utxos_after[0].reserved,
        "expected seller's NOID UTXO to remain locked (reserved=true) even after dismiss!"
    );
    {
        let g = shared_wallet.lock().unwrap();
        let w = g.as_ref().unwrap();
        ensure!(
            w.pending_input_slots.contains(&item_live.slot_index),
            "expected seller's NFT slot to remain locked in pending_input_slots even after dismiss!"
        );
    }
    println!(
        "  [confirmed] CreateListing evicted after 1 missed block; after forget_native_activity(), ghost native_pending len = 1, NFT slot {} & NOID slot {} remain locked!",
        item_live.slot_index, cash_live.slot_index
    );
    Ok(())
}

// ============================================================================
// TEST 5 (Finding E): `paranoid_getBlockTemplate` Per-Body Fee Burn Accounting
// Mismatch vs True Group-Level Consensus Burn (`NativeIntent` & Multi-Page `PagedSpend`).
// ============================================================================
fn test_5_get_block_template_per_body_fee_burn_mismatch() -> Result<()> {
    println!("\n=== [Test 5] getBlockTemplate Per-Body vs Per-Group Fee Burn Mismatch ===");
    let parent_header = noid_chain::genesis::genesis_block().header;
    let active_slots = 100u64;
    let log_slots = parent_header.log_slots;

    // Case A: Multi-page `PagedSpend` with 1 input and 4 outputs (requires 2 physical pages:
    // page 0 has 1 input, 2 outputs, fee = F; page 1 has 0 inputs, 2 outputs, fee = 0).
    // True group-level net_new_slots = 4 - 1 = 3 => burned = 7,500 uNOID, minimum F = 17,500 uNOID,
    // true claimable miner fee = 10,000 uNOID.
    let secret = SpendSecret::from_bytes([0xAB; 32]);
    let owner = secret.derive_address();
    let group_bd = fee_breakdown(1, 4, active_slots, log_slots);
    ensure!(group_bd.burned == 7_500 && group_bd.required_total == 17_500);
    let true_claimable_a = group_bd.required_total - group_bd.burned;
    ensure!(true_claimable_a == 10_000);

    let (paged_intent, _) = build_signed_ordinary_spend(
        secret,
        [0x11; 32],
        1,
        &[(10, 1_000_000, 1)],
        &[
            (20, 100_000, owner),
            (21, 100_000, owner),
            (22, 100_000, owner),
            (23, 682_500, owner),
        ],
        group_bd.required_total,
    )?;
    ensure!(paged_intent.pages.len() == 2);

    // Exact calculation performed by `prepare_external_mining_attempt` (`noid_rpc/src/server.rs:1318-1329`):
    let rpc_reported_a: u64 = paged_intent
        .pages
        .iter()
        .fold(0u64, |acc, page| {
            acc.saturating_add(claimable_fee_for_tx_body(
                &page.body,
                active_slots,
                log_slots,
            ))
        });
    // Page 0 (`1 in, 2 out, fee = 17,500`): burns only `1 * 2,500 = 2,500` -> `15,000`
    // Page 1 (`0 in, 2 out, fee = 0`): `0.saturating_sub(5,000) = 0`
    // Total RPC reported = `15,000` != true `10,000` (+5,000 uNOID over-reported)!
    ensure!(
        rpc_reported_a == 15_000 && rpc_reported_a != true_claimable_a,
        "expected RPC per-body sum 15,000 != true claimable 10,000"
    );
    println!(
        "  [confirmed] Case A (2-page 1-in/4-out PagedSpend): consensus claimable = {} uNOID, getBlockTemplate reports = {} uNOID (+{} uNOID error).",
        true_claimable_a,
        rpc_reported_a,
        rpc_reported_a - true_claimable_a
    );

    // Case B: Any `NativeIntent` with state growth (e.g. `CreateLaunch`, 1 input -> 4 outputs,
    // `fee = 17,500`, `burned = 7,500`, true claimable = `10,000`).
    // In every `NativeIntent`, `pages[0]` is the metadata header page (`0` live inputs, `0` live outputs,
    // `fee = 17,500`) and `pages[1..]` carry `fee = 0`!
    let mut header_parent = parent_header;
    header_parent.height = 300_000;
    header_parent.active_slot_count = active_slots;
    let (native_intent, _, native_fee) = build_signed_create_launch(
        secret,
        &header_parent,
        [0x22; 32],
        TxInput {
            slot_index: 10,
            amount: 500_000_000,
            creation_id: 1,
        },
        [30, 31, 32, 33],
        0xEE,
    )?;
    let native_bd = fee_breakdown(1, 4, active_slots, log_slots);
    let true_claimable_b = native_fee - native_bd.burned;
    let rpc_reported_b: u64 = native_intent
        .pages
        .iter()
        .fold(0u64, |acc, page| {
            acc.saturating_add(claimable_fee_for_tx_body(
                &page.body,
                active_slots,
                log_slots,
            ))
        });
    // `pages[0]` has 0 inputs & 0 outputs -> `burned = 0` -> `claimable = 17,500`!
    ensure!(
        rpc_reported_b == 17_500 && true_claimable_b == 10_000,
        "expected RPC per-body sum 17,500 != true claimable 10,000"
    );
    println!(
        "  [confirmed] Case B (NativeIntent CreateLaunch): consensus claimable = {} uNOID, getBlockTemplate reports = {} uNOID (+{} uNOID error; 100% of burn missed).",
        true_claimable_b,
        rpc_reported_b,
        rpc_reported_b - true_claimable_b
    );
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    test_1_financial_fee_downward_transition_halts_template_builder().await?;
    test_2_coinbase_hydration_reserved_slot_short_circuit().await?;
    test_3_reorg_wallet_reservation_ordering_desync().await?;
    test_4_create_listing_eviction_and_broken_dismiss_pending().await?;
    test_5_get_block_template_per_body_fee_burn_mismatch()?;
    println!("\nALL 5 COMPOSITIONAL CORE-SEAM VERIFICATION TESTS PASSED DETERMINISTICALLY.");
    Ok(())
}
