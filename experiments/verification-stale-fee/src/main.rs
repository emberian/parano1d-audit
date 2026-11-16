use anyhow::{Context, Result};
use noid_chain::block_header::{semantic_header_id, BlockHeader};
use noid_chain::consensus::forks::ACTIVE_SCHEDULE;
use noid_chain::consensus::params::owner_authorization_version;
use noid_chain::consensus::template::{
    build_node_owned_block_template, build_node_owned_block_template_with_native_data,
};
use noid_chain::consensus::{fee_breakdown, next_target, validate_block_consensus};
use noid_chain::mempool::{BlockSelectionBudget, NativeFinancialBudget};
use noid_chain::{block_id, AcceptedBlockBundle, HistoryStepTerminalMetadata, MdbxChainContext};
use noid_gkr::wallet_authorization::prove_native_financial_authorization;
use noid_gkr::OwnerAuthWitness;
use noid_mempool::{AsyncMempool, ChainView, MempoolConfig};
use noid_poseidon2b::primitives::{derive_address, Address, SpendSecret};
use noid_tx::asset_backing::TokenDenomination;
use noid_tx::native_assets::financial::FinancialOperation;
use noid_tx::native_assets::launch::{
    launch_supply_from_whole, LaunchDefinition, LaunchOpening, LaunchState,
};
use noid_tx::native_assets::nft_market::NftRoundContext;
use noid_tx::native_assets::transition::{NativeContext, NativeInputClaim, NativeOutputClaim};
use noid_tx::native_assets::wire::NativeIntent;
use noid_tx::native_assets::{
    FungibleDefinition, GenesisAnchor, IssuanceRoute, NativeOpening, TokenReserve, TOKEN_DECIMALS,
};
use noid_tx::{Transaction, TxInput, TxOutput};

fn mine_empty(ctx: &mut MdbxChainContext, miner: Address) -> Result<()> {
    let parent = *ctx.tip_header();
    let finalized = ctx.finalized_active_counts()?;
    let prev_ts = ctx.prev_timestamps()?;
    let anchor = ctx.anchor_info()?;
    let ts = parent.timestamp + ACTIVE_SCHEDULE.block_time(parent.height + 1);
    let target = next_target(
        anchor.anchor_height,
        anchor.anchor_timestamp,
        &anchor.anchor_target,
        parent.height + 1,
        parent.timestamp,
        ts,
    );
    let (template, _) =
        build_node_owned_block_template(&parent, &ctx.state, &finalized, vec![], miner, ts, target)
            .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    let mut block = template.into_block(0);
    block.header.nonce = noid_chain::consensus::pow::search_pow(&block.header, 0, 10_000_000)
        .context("10 million PoW trials did not find a block")?;
    let metadata = HistoryStepTerminalMetadata::new(
        block.header.height,
        semantic_header_id(&block.header),
        0,
    )?;
    let mut terminal = metadata.encode_prefix().to_vec();
    terminal.push(1);
    let bundle = AcceptedBlockBundle::try_from_parts(block.to_bytes(), terminal)?;
    ctx.apply_next_block(
        &bundle,
        ts + 1,
        |blk, state| {
            let mut next = state.clone();
            let root = validate_block_consensus(
                blk,
                &parent,
                &prev_ts,
                &finalized,
                ts + 1,
                &anchor,
                &mut next,
            )?;
            *state = next;
            Ok::<_, noid_chain::consensus::ConsensusError>(root)
        },
        |_| Ok(()),
    )?;
    Ok(())
}

fn budget(financial: bool) -> BlockSelectionBudget {
    BlockSelectionBudget {
        pages: 63,
        live_inputs: 504,
        contract_calls: 63,
        native_headers: if financial { 16 } else { 0 },
        native_records: if financial { 64 } else { 0 },
        owner_authorizations: 63,
        financial: financial.then_some(NativeFinancialBudget {
            actions: 256,
            definitions: 20,
            markets: 4,
            policies: 16,
            funding: 64,
            owner_operations: true,
        }),
    }
}

fn create_launch(
    owner_secret: SpendSecret,
    inputs: Vec<TxInput>,
    parent: &BlockHeader,
    epoch_anchor: [u8; 32],
) -> Result<NativeIntent> {
    let owner = derive_address(&owner_secret);
    let height = parent.height + 1;
    let supply = launch_supply_from_whole(100_000).context("launch supply")?;
    let deposit = TokenDenomination::new(TOKEN_DECIMALS)
        .map_err(|error| anyhow::anyhow!("{error:?}"))?
        .minimum_backing(supply)
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    let fee = fee_breakdown(
        inputs.len() as u64,
        4,
        parent.active_slot_count,
        parent.log_slots,
    )
    .required_total;
    let genesis = GenesisAnchor {
        chain: noid_chain::state::native_assets::native_chain_id(),
        owner,
        input: inputs[0],
    };
    let token =
        FungibleDefinition::from_genesis(&genesis, supply, IssuanceRoute::Launchpad, [0x71; 32])
            .context("fungible definition")?;
    let launch = LaunchDefinition::from_genesis(&genesis, token).context("launch definition")?;
    let launch_state = LaunchState::initial(launch).context("launch state")?;
    let reserve = TokenReserve::from_definition(&token, deposit).context("token reserve")?;
    let definition = NativeOpening::Launch(LaunchOpening::Definition(launch));
    let token_opening = NativeOpening::FungibleDefinition(token);
    let reserve_opening = NativeOpening::TokenReserve(reserve);
    let state_opening = NativeOpening::Launch(LaunchOpening::State(launch_state));
    let outputs = vec![
        NativeOutputClaim {
            output: TxOutput {
                slot_index: 600,
                amount: 0,
                owner: token_opening.root(),
            },
            opening: Some(token_opening),
        },
        NativeOutputClaim {
            output: TxOutput {
                slot_index: 601,
                amount: deposit,
                owner: reserve_opening.root(),
            },
            opening: Some(reserve_opening),
        },
        NativeOutputClaim {
            output: TxOutput {
                slot_index: 602,
                amount: 0,
                owner: state_opening.root(),
            },
            opening: Some(state_opening),
        },
        NativeOutputClaim {
            output: TxOutput {
                slot_index: 603,
                amount: inputs.iter().map(|input| input.amount).sum::<u64>() - deposit - fee,
                owner,
            },
            opening: None,
        },
    ];
    let round = NftRoundContext::new(height, block_id(parent), parent.alloc_counter)?;
    let context = NativeContext {
        chain: noid_chain::state::native_assets::native_chain_id(),
        height,
        authority: owner,
        fee,
    };
    let mut intent = NativeIntent::from_financial_claims(
        &context,
        epoch_anchor,
        FinancialOperation::CreateLaunch {
            funding_input: 0,
            definition: definition.root(),
        },
        round,
        inputs
            .into_iter()
            .map(|input| NativeInputClaim {
                input,
                root: owner,
                opening: None,
            })
            .collect(),
        outputs,
        |root| (root == definition.root()).then_some(definition),
    )
    .context("financial claims")?;
    let proof = prove_native_financial_authorization(
        &intent,
        context.chain,
        round,
        OwnerAuthWitness::new(owner_secret),
        owner_authorization_version(height),
    )?;
    intent.authorization_bytes = proof.to_bytes()?;
    Ok(intent)
}

fn openings(intent: &NativeIntent) -> Vec<NativeOpening> {
    intent
        .input_openings
        .iter()
        .chain(&intent.output_openings)
        .flatten()
        .chain(&intent.dependencies)
        .copied()
        .collect()
}

#[tokio::main]
async fn main() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut ctx = MdbxChainContext::open_or_create(&dir.path().join("chain"))?;
    let launch_secret = SpendSecret::from_bytes([0x22; 32]);
    let launch_owner = derive_address(&launch_secret);
    for h in 1..=23 {
        mine_empty(&mut ctx, launch_owner)?;
        eprintln!("accepted local block {h}");
    }
    let actual_parent = *ctx.tip_header();
    anyhow::ensure!(actual_parent.height == 23, "unexpected local height");
    let genesis_header = ctx
        .store
        .get_header(0)?
        .context("genesis epoch anchor header")?;
    let anchor = block_id(&genesis_header);
    let launch_utxos = ctx
        .store
        .get_verified_utxos_by_owner(&launch_owner.0)?
        .utxos
        .into_iter()
        .take(3)
        .map(|utxo| TxInput {
            slot_index: utxo.slot_index,
            amount: utxo.amount,
            creation_id: utxo.creation_id,
        })
        .collect::<Vec<_>>();
    anyhow::ensure!(launch_utxos.len() == 3, "need three coinbases");

    // Only the fixture's pre-admission ChainView/header are synthetic.
    // Production genesis has 2^24 slots. Populating half of them to cross the
    // 50% threshold is outside the scope of a bounded local chain fixture.
    // The state itself remains the accepted height-23 State, so this is a
    // component seam reproduction, not a valid threshold-crossing chain.
    let threshold = 1u64 << (actual_parent.log_slots - 1);
    let mut synthetic_parent = actual_parent;
    synthetic_parent.active_slot_count = threshold;
    let mut synthetic_headers = ChainView::from_mdbx(&ctx).recent_headers;
    synthetic_headers.insert(0, genesis_header);
    synthetic_headers.insert(actual_parent.height, synthetic_parent);
    let high_view = ChainView::new(
        actual_parent.height,
        synthetic_headers,
        threshold,
        ctx.state.state.clone(),
    );
    let high_fee = fee_breakdown(3, 4, threshold, synthetic_parent.log_slots).required_total;
    let low_fee = fee_breakdown(
        3,
        4,
        actual_parent.active_slot_count,
        actual_parent.log_slots,
    )
    .required_total;
    anyhow::ensure!(high_fee > low_fee, "fee pressure did not change");

    let stale = create_launch(
        launch_secret,
        launch_utxos.clone(),
        &synthetic_parent,
        anchor,
    )?;
    anyhow::ensure!(stale.pages[0].body.fee == high_fee, "high fee mismatch");
    let config = MempoolConfig::default()
        .with_capacity(5)
        .with_v2_block_budgets([budget(false), budget(false)])
        .with_v21_native_budget(budget(true));
    let pool = AsyncMempool::new(high_view, config.clone());
    pool.submit_encoded(stale.to_bytes()?).await?;
    anyhow::ensure!(pool.len().await == 1, "high-pressure intent not admitted");

    // This block is accepted through the real consensus/MDBX path. Only the
    // prior high-pressure view was synthetic.
    mine_empty(&mut ctx, launch_owner)?;
    eprintln!("accepted local block 24");
    let parent = *ctx.tip_header();
    anyhow::ensure!(parent.height == 24, "real child not accepted");
    let low_view = ChainView::from_mdbx(&ctx);
    pool.on_new_block(&[], parent.height, low_view.clone())
        .await;
    anyhow::ensure!(pool.len().await == 1, "stale exact-fee intent was evicted");
    let selected = pool
        .select_for_v2_mining(budget(true), None, parent.height + 1, anchor)
        .await?;
    anyhow::ensure!(selected.entries.len() == 1, "stale intent not selected");
    let selected_intent = selected.entries[0]
        .native_intent
        .as_ref()
        .context("selected native intent")?;
    let round = NftRoundContext::new(parent.height + 1, block_id(&parent), parent.alloc_counter)?;
    selected_intent
        .checked_financial_owner(noid_chain::state::native_assets::native_chain_id(), round)?;

    let finalized = ctx.finalized_active_counts()?;
    let ts = parent.timestamp + ACTIVE_SCHEDULE.block_time(parent.height + 1);
    let difficulty_anchor = ctx.anchor_info()?;
    let target = next_target(
        difficulty_anchor.anchor_height,
        difficulty_anchor.anchor_timestamp,
        &difficulty_anchor.anchor_target,
        parent.height + 1,
        parent.timestamp,
        ts,
    );
    let stale_txs: Vec<_> = selected.entries[0]
        .pages
        .iter()
        .map(|page| Transaction::new(page.body.clone()))
        .collect();
    let stale_result = build_node_owned_block_template_with_native_data(
        &parent,
        &ctx.state,
        &finalized,
        stale_txs,
        launch_owner,
        ts,
        target,
        &selected.pending_outputs,
        &openings(&stale),
        &[],
    );
    let stale_error = match stale_result {
        Err(error) => error,
        Ok(_) => anyhow::bail!("stale exact-fee intent unexpectedly built a template"),
    };
    anyhow::ensure!(
        format!("{stale_error:?}").contains("Fee"),
        "unexpected template error: {stale_error:?}"
    );

    let (_empty_template, _empty_prepared) = build_node_owned_block_template_with_native_data(
        &parent,
        &ctx.state,
        &finalized,
        Vec::new(),
        launch_owner,
        ts,
        target,
        &Default::default(),
        &[],
        &[],
    )
    .map_err(|error| anyhow::anyhow!("empty template control failed: {error:?}"))?;

    // Positive control: the same authentic parent and UTXOs, freshly signed
    // with its exact lower fee, must admit and build through the same path.
    let fresh = create_launch(
        SpendSecret::from_bytes([0x22; 32]),
        launch_utxos,
        &parent,
        anchor,
    )?;
    anyhow::ensure!(fresh.pages[0].body.fee == low_fee, "low fee mismatch");
    let fresh_pool = AsyncMempool::new(low_view, config);
    fresh_pool.submit_encoded(fresh.to_bytes()?).await?;
    let fresh_selected = fresh_pool
        .select_for_v2_mining(budget(true), None, parent.height + 1, anchor)
        .await?;
    anyhow::ensure!(
        fresh_selected.entries.len() == 1,
        "fresh intent not selected"
    );
    let fresh_txs: Vec<_> = fresh_selected.entries[0]
        .pages
        .iter()
        .map(|page| Transaction::new(page.body.clone()))
        .collect();
    let (_fresh_template, _fresh_prepared) = build_node_owned_block_template_with_native_data(
        &parent,
        &ctx.state,
        &finalized,
        fresh_txs,
        launch_owner,
        ts,
        target,
        &fresh_selected.pending_outputs,
        &openings(&fresh),
        &[],
    )
    .map_err(|error| anyhow::anyhow!("fresh exact-fee template failed: {error:?}"))?;

    println!(
        "PASS: synthetic high-pressure admission fee {high_fee}; real accepted block 24; stale retained and selected at low-pressure fee {low_fee}; stale template rejected {stale_error:?}; empty and fresh exact-fee templates built"
    );
    Ok(())
}
