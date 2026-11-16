use anyhow::{Context, Result};
use noid_chain::block::native_assets::{
    financial::FinancialHeaderWindow, materialize_accepted_block_state_with_native_data,
};
use noid_chain::block_header::{semantic_header_id, BlockHeader};
use noid_chain::consensus::claimable_fee_for_tx_body;
use noid_chain::consensus::development_allocation::miner_subsidy_at_height;
use noid_chain::consensus::forks::ACTIVE_SCHEDULE;
use noid_chain::consensus::params::owner_authorization_version;
use noid_chain::consensus::template::build_node_owned_block_template;
use noid_chain::consensus::template::{
    build_node_owned_block_template_with_native_openings, BlockTemplate,
};
use noid_chain::consensus::validate_block_checks;
use noid_chain::consensus::{fee_breakdown, next_target, validate_block_consensus};
use noid_chain::{block_id, AcceptedBlockBundle, HistoryStepTerminalMetadata, MdbxChainContext};
use noid_gkr::wallet_authorization::{
    prove_native_financial_authorization, prove_paged_spend_authorization_for_version,
};
use noid_gkr::OwnerAuthWitness;
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
use noid_tx::Transaction;
use noid_tx::{
    output_bitmap_bit, PagedSpendIntent, TxBody, TxInput, TxOutput, TxPage, PAGED_SPEND_END_BIT,
    PAGED_SPEND_START_BIT, TX_INPUTS, TX_OUTPUTS,
};
use std::collections::HashSet;

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

fn ordinary_intent(
    secret: SpendSecret,
    input: TxInput,
    output_slot: u32,
    anchor: [u8; 32],
    height: u64,
) -> Result<PagedSpendIntent> {
    let owner = derive_address(&secret);
    let fee = 100_000;
    let mut inputs = [TxInput::dummy(); TX_INPUTS];
    inputs[0] = input;
    let mut outputs = [TxOutput::dummy(); TX_OUTPUTS];
    outputs[0] = TxOutput {
        slot_index: output_slot,
        amount: input.amount - fee,
        owner,
    };
    let body = TxBody {
        epoch_anchor: anchor,
        fee,
        input_owner: owner,
        inputs,
        outputs,
        validity_bitmap: 1 | output_bitmap_bit(0) | PAGED_SPEND_START_BIT | PAGED_SPEND_END_BIT,
        is_coinbase: false,
    };
    let pages = vec![TxPage::new(body)?];
    let proof = prove_paged_spend_authorization_for_version(
        &pages,
        OwnerAuthWitness::new(secret),
        owner_authorization_version(height),
    )?;
    Ok(PagedSpendIntent::new(pages, proof.to_bytes()?)?)
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

fn build_selected(
    ctx: &MdbxChainContext,
    miner: Address,
    pages: Vec<Transaction>,
    openings: &[NativeOpening],
) -> Result<BlockTemplate> {
    let parent = *ctx.tip_header();
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
    let (template, _) = build_node_owned_block_template_with_native_openings(
        &parent,
        &ctx.state,
        &ctx.finalized_active_counts()?,
        pages,
        miner,
        ts,
        target,
        &HashSet::new(),
        openings,
    )
    .map_err(|error| anyhow::anyhow!("template build: {error:?}"))?;
    Ok(template)
}

fn check_case(
    label: &str,
    ctx: &MdbxChainContext,
    template: BlockTemplate,
    expected_pages: usize,
    openings: &[NativeOpening],
) -> Result<u64> {
    let parent = *ctx.tip_header();
    anyhow::ensure!(
        template.txs.len() == expected_pages,
        "{label}: template did not select all pages"
    );
    let rpc_reported: u64 = template
        .txs
        .iter()
        .map(|tx| claimable_fee_for_tx_body(&tx.body, parent.active_slot_count, parent.log_slots))
        .sum();
    let subsidy = miner_subsidy_at_height(template.height, template.log_slots);
    let coinbase = template
        .coinbase
        .body
        .live_outputs()
        .next()
        .context("coinbase output")?
        .1
        .amount;
    let coinbase_delta = coinbase
        .checked_sub(subsidy)
        .context("coinbase below subsidy")?;
    let mut block = template.into_block(0);
    block.header.nonce = noid_chain::consensus::pow::search_pow(&block.header, 0, 10_000_000)
        .context("10 million PoW trials did not find block")?;
    let mut next = ctx.state.clone();
    if openings.is_empty() {
        validate_block_consensus(
            &block,
            &parent,
            &ctx.prev_timestamps()?,
            &ctx.finalized_active_counts()?,
            block.header.timestamp + 1,
            &ctx.anchor_info()?,
            &mut next,
        )?;
    } else {
        validate_block_checks(
            &block,
            &parent,
            &ctx.prev_timestamps()?,
            &ctx.finalized_active_counts()?,
            block.header.timestamp + 1,
            &ctx.anchor_info()?,
        )?;
        let financial = FinancialHeaderWindow::new(parent, &[])
            .map_err(|error| anyhow::anyhow!("financial window: {error:?}"))?;
        materialize_accepted_block_state_with_native_data(
            &mut next,
            &block,
            &financial,
            &[],
            |root| {
                openings
                    .iter()
                    .find(|opening| opening.root() == root)
                    .copied()
            },
        )
        .map_err(|error| anyhow::anyhow!("native materialization: {error:?}"))?;
    }
    let fee = block
        .transactions
        .iter()
        .skip(1)
        .map(|tx| tx.body.fee)
        .sum::<u64>();
    println!("{label}: selected_pages={expected_pages} total_fee={fee} rpc_reported={rpc_reported} coinbase_delta={coinbase_delta} overreport={}", rpc_reported.saturating_sub(coinbase_delta));
    Ok(rpc_reported - coinbase_delta)
}

fn main() -> Result<()> {
    let chain_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".local-chain");
    let mut ctx = MdbxChainContext::open_or_create(&chain_dir)?;
    let ordinary_secret = SpendSecret::from_bytes([0x11; 32]);
    let launch_secret = SpendSecret::from_bytes([0x22; 32]);
    let ordinary_owner = derive_address(&ordinary_secret);
    let launch_owner = derive_address(&launch_secret);
    for h in (ctx.tip_height() + 1)..=23 {
        mine_empty(&mut ctx, if h == 2 { launch_owner } else { ordinary_owner })?;
        eprintln!("accepted local block {h}");
    }
    // A reopened MDBX context retains compact roots but evicts nonzero raw
    // segments. The direct template builder requires those columns resident.
    for segment in 0..ctx.state.state.num_segments() {
        let id = u16::try_from(segment)?;
        if ctx.state.state.is_evicted(id) {
            let (_, columns) = ctx
                .store
                .get_segment(id)?
                .context("evicted segment missing")?;
            ctx.state.restore_evicted_segment(id, columns)?;
        }
    }
    let parent = *ctx.tip_header();
    anyhow::ensure!(parent.height == 23, "unexpected parent height");
    let anchor = block_id(&ctx.store.get_header(0)?.context("genesis header")?);

    let ordinary_utxo = ctx
        .store
        .get_verified_utxos_by_owner(&launch_owner.0)?
        .utxos
        .into_iter()
        .min_by_key(|utxo| utxo.creation_id)
        .context("ordinary owner UTXO")?;
    let ordinary = ordinary_intent(
        launch_secret,
        TxInput {
            slot_index: ordinary_utxo.slot_index,
            amount: ordinary_utxo.amount,
            creation_id: ordinary_utxo.creation_id,
        },
        500,
        anchor,
        24,
    )?;
    let ordinary_pages: Vec<_> = ordinary
        .pages
        .iter()
        .map(|page| Transaction::new(page.body.clone()))
        .collect();
    anyhow::ensure!(
        ordinary_pages.len() == 1,
        "ordinary control should occupy one page"
    );
    let ordinary_template = build_selected(&ctx, launch_owner, ordinary_pages.clone(), &[])?;
    anyhow::ensure!(
        check_case(
            "ordinary_control",
            &ctx,
            ordinary_template,
            ordinary_pages.len(),
            &[]
        )? == 0,
        "ordinary control diverged"
    );

    let mut launch_utxos = ctx
        .store
        .get_verified_utxos_by_owner(&ordinary_owner.0)?
        .utxos
        .into_iter()
        .collect::<Vec<_>>();
    launch_utxos.sort_by_key(|utxo| utxo.creation_id);
    anyhow::ensure!(launch_utxos.len() >= 3, "need three mature launch inputs");
    let launch = create_launch(
        ordinary_secret,
        launch_utxos
            .iter()
            .take(3)
            .map(|utxo| TxInput {
                slot_index: utxo.slot_index,
                amount: utxo.amount,
                creation_id: utxo.creation_id,
            })
            .collect(),
        &parent,
        anchor,
    )?;
    let launch_pages: Vec<_> = launch
        .pages
        .iter()
        .map(|page| Transaction::new(page.body.clone()))
        .collect();
    anyhow::ensure!(
        launch_pages.len() > 1,
        "native group should occupy multiple pages"
    );
    anyhow::ensure!(
        launch_pages[0].body.fee > 0
            && launch_pages[0].body.live_input_count() == 0
            && launch_pages[0].body.live_output_count() == 0
            && launch_pages.iter().skip(1).all(|page| page.body.fee == 0),
        "native fee/header page layout changed"
    );
    let openings: Vec<_> = launch
        .input_openings
        .iter()
        .chain(launch.output_openings.iter())
        .filter_map(|opening| *opening)
        .chain(launch.dependencies.iter().copied())
        .collect();
    let logical = launch.resource_facts()?;
    let burned = fee_breakdown(
        u64::from(logical.live_inputs),
        u64::from(logical.live_outputs),
        parent.active_slot_count,
        parent.log_slots,
    )
    .burned;
    anyhow::ensure!(burned > 0, "fixture must have positive state-growth burn");
    let launch_template = build_selected(&ctx, ordinary_owner, launch_pages.clone(), &openings)?;
    let overreport = check_case(
        "native_create_launch",
        &ctx,
        launch_template,
        launch_pages.len(),
        &openings,
    )?;
    anyhow::ensure!(
        overreport == burned,
        "overreport {overreport} != logical burn {burned}"
    );
    println!("PASS: exact RPC fee expression exceeds accepted-template coinbase subsidy delta by {overreport} micronoid");
    Ok(())
}
