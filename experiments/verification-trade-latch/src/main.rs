use anyhow::{Context, Result};
use noid_chain::block_header::{semantic_header_id, BlockHeader};
use noid_chain::consensus::forks::ACTIVE_SCHEDULE;
use noid_chain::consensus::params::owner_authorization_version;
use noid_chain::consensus::template::build_node_owned_block_template;
use noid_chain::consensus::{fee_breakdown, next_target, validate_block_consensus};
use noid_chain::mempool::{BlockSelectionBudget, NativeFinancialBudget};
use noid_chain::{block_id, AcceptedBlockBundle, HistoryStepTerminalMetadata, MdbxChainContext};
use noid_gkr::wallet_authorization::{
    prove_native_financial_authorization, prove_native_trade_authorization,
    prove_paged_spend_authorization_for_version,
};
use noid_gkr::OwnerAuthWitness;
use noid_mempool::{AsyncMempool, ChainView, MempoolConfig, SubmitError};
use noid_poseidon2b::primitives::{derive_address, Address, SpendSecret};
use noid_tx::asset_backing::TokenDenomination;
use noid_tx::native_assets::financial::FinancialOperation;
use noid_tx::native_assets::launch::LaunchSide;
use noid_tx::native_assets::launch::{
    launch_supply_from_whole, LaunchDefinition, LaunchOpening, LaunchState,
};
use noid_tx::native_assets::market::MARKET_QUANTITY_MAX;
use noid_tx::native_assets::nft_market::NftRoundContext;
use noid_tx::native_assets::trade::{wire::TradeOwnerIntent, TradeIntent, TradePolicy};
use noid_tx::native_assets::transition::{NativeContext, NativeInputClaim, NativeOutputClaim};
use noid_tx::native_assets::wire::NativeIntent;
use noid_tx::native_assets::{
    AssetId, FungibleDefinition, GenesisAnchor, IssuanceRoute, NativeOpening, TokenReserve,
    TOKEN_DECIMALS,
};
use noid_tx::{
    output_bitmap_bit, OwnerAuthorizationVersion, PagedSpendIntent, Transaction, TxBody, TxInput,
    TxOutput, TxPage, PAGED_SPEND_END_BIT, PAGED_SPEND_START_BIT, TX_INPUTS, TX_OUTPUTS,
};

fn mine_block(ctx: &mut MdbxChainContext, miner: Address, txs: Vec<Transaction>) -> Result<()> {
    let expected_user_ids = txs.iter().map(Transaction::txid).collect::<Vec<_>>();
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
        build_node_owned_block_template(&parent, &ctx.state, &finalized, txs, miner, ts, target)
            .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    let mut block = template.into_block(0);
    for expected in &expected_user_ids {
        anyhow::ensure!(
            block.transactions.iter().any(|tx| tx.txid() == *expected),
            "block template omitted requested ordinary transaction {expected:?}"
        );
    }
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

fn unindexed_trade(
    secret: SpendSecret,
    input: TxInput,
    epoch_anchor: [u8; 32],
    candidate_height: u64,
) -> Result<TradeOwnerIntent> {
    let owner = derive_address(&secret);
    let intent = TradeIntent {
        policy: TradePolicy {
            chain: noid_chain::state::native_assets::native_chain_id(),
            epoch_anchor,
            owner,
            asset: AssetId([0xaa; 32]),
            recipient: owner,
            side: LaunchSide::Buy,
            gross_input: 1_000,
            minimum_output: MARKET_QUANTITY_MAX,
            fee_limit: 25_000,
            valid_from: candidate_height,
            valid_until: u64::MAX,
        },
        funding: vec![NativeInputClaim {
            input,
            root: owner,
            opening: None,
        }],
    };
    let proof = prove_native_trade_authorization(
        &intent,
        noid_chain::state::native_assets::native_chain_id(),
        epoch_anchor,
        candidate_height,
        OwnerAuthWitness::new(secret),
        OwnerAuthorizationVersion::V4,
    )?;
    Ok(TradeOwnerIntent {
        intent,
        authorization_bytes: proof.to_bytes()?,
    })
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

#[tokio::main]
async fn main() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut ctx = MdbxChainContext::open_or_create(&dir.path().join("chain"))?;
    let trade_secret = SpendSecret::from_bytes([0x33; 32]);
    let launch_secret = SpendSecret::from_bytes([0x22; 32]);
    let ordinary_owner = derive_address(&SpendSecret::from_bytes([0x11; 32]));
    let trade_owner = derive_address(&trade_secret);
    let launch_owner = derive_address(&launch_secret);
    for h in 1..=23 {
        let miner = if h <= 2 {
            ordinary_owner
        } else if h <= 5 {
            trade_owner
        } else {
            launch_owner
        };
        mine_block(&mut ctx, miner, vec![])?;
    }
    let parent = *ctx.tip_header();
    assert_eq!(parent.height, 23);
    let anchor = block_id(
        &ctx.store
            .get_header(0)?
            .context("genesis epoch anchor header")?,
    );
    let config = MempoolConfig::default()
        .with_capacity(6)
        .with_v2_block_budgets([budget(false), budget(false)])
        .with_v21_native_budget(budget(true));
    let view = ChainView::from_mdbx(&ctx);
    let congested = AsyncMempool::new(view.clone(), config.clone());
    let trade_utxos = ctx.store.get_verified_utxos_by_owner(&trade_owner.0)?.utxos;
    anyhow::ensure!(trade_utxos.len() == 3, "trade owner needs three coinbases");
    for (i, utxo) in trade_utxos.iter().enumerate() {
        let trade = unindexed_trade(
            SpendSecret::from_bytes([0x33; 32]),
            TxInput {
                slot_index: utxo.slot_index,
                amount: utxo.amount,
                creation_id: utxo.creation_id,
            },
            anchor,
            24,
        )?;
        congested.submit_encoded(trade.to_bytes()?).await?;
        println!(
            "authentic V4 trade admitted: entry {} / 6, floor {}",
            i + 1,
            congested.fee_floor().await,
        );
    }
    assert_eq!(congested.len().await, 3);
    assert_eq!(congested.fee_floor().await, 5_000);

    let ordinary_utxos = ctx
        .store
        .get_verified_utxos_by_owner(&ordinary_owner.0)?
        .utxos;
    anyhow::ensure!(
        ordinary_utxos.len() == 2,
        "ordinary owner needs two coinbases"
    );
    let mut ordinary_ids = Vec::new();
    let mut ordinary_txs = Vec::new();
    for (i, utxo) in ordinary_utxos.iter().enumerate() {
        let ordinary = ordinary_intent(
            SpendSecret::from_bytes([0x11; 32]),
            TxInput {
                slot_index: utxo.slot_index,
                amount: utxo.amount,
                creation_id: utxo.creation_id,
            },
            500 + i as u32,
            anchor,
            24,
        )?;
        let admitted_id = congested.submit_encoded(ordinary.to_bytes()?).await?;
        anyhow::ensure!(
            noid_tx::validate_paged_spend(&ordinary.pages)
                .map_err(|error| anyhow::anyhow!("{error:?}"))?
                .logical_txid
                == admitted_id,
            "mempool confirmation ID differs from logical PagedSpend ID"
        );
        ordinary_ids.push(admitted_id);
        ordinary_txs.push(Transaction::new(ordinary.pages[0].body.clone()));
    }
    assert_eq!(congested.len().await, 5);
    assert_eq!(congested.fee_floor().await, 90_000);

    let selected = congested.select_for_block_at_anchor(6, anchor).await;
    assert_eq!(
        selected.len(),
        2,
        "trade policies must not enter ordinary selection"
    );
    anyhow::ensure!(
        selected
            .iter()
            .all(|entry| ordinary_ids.contains(&entry.logical_txid)),
        "selection contains nonordinary entry"
    );

    mine_block(&mut ctx, launch_owner, ordinary_txs)?;
    assert_eq!(ctx.tip_height(), 24);
    let updated_view = ChainView::from_mdbx(&ctx);
    congested
        .on_new_block(&ordinary_ids, 24, updated_view.clone())
        .await;
    assert_eq!(
        congested.len().await,
        3,
        "all three funded trade policies survive accepted block"
    );
    assert_eq!(
        congested.fee_floor().await,
        90_000,
        "trade occupancy latches historical ordinary fee"
    );
    assert!(congested
        .select_for_block_at_anchor(6, anchor)
        .await
        .is_empty());

    let launch_utxos = ctx
        .store
        .get_verified_utxos_by_owner(&launch_owner.0)?
        .utxos
        .into_iter()
        .take(8)
        .map(|utxo| TxInput {
            slot_index: utxo.slot_index,
            amount: utxo.amount,
            creation_id: utxo.creation_id,
        })
        .collect::<Vec<_>>();
    anyhow::ensure!(
        launch_utxos.len() == 8,
        "launch owner needs eight coinbases"
    );
    let launch = create_launch(launch_secret, launch_utxos, ctx.tip_header(), anchor)?;
    let exact_fee = launch.pages[0].body.fee;
    assert!(exact_fee < congested.fee_floor().await);
    let bytes = launch.to_bytes()?;
    let fresh = AsyncMempool::new(updated_view, config);
    fresh.submit_encoded(bytes.clone()).await?;
    assert_eq!(fresh.len().await, 1);
    let err = congested
        .submit_encoded(bytes)
        .await
        .expect_err("raised local floor should exclude exact-fee financial intent");
    match err {
        SubmitError::Consensus(noid_chain::consensus::ConsensusError::BelowMinFee {
            required,
            actual,
        }) => {
            assert_eq!(required, 90_000);
            assert_eq!(actual, exact_fee);
        }
        other => anyhow::bail!("unexpected admission result: {other:?}"),
    }
    println!(
        "PASS: accepted block 24 confirmed both ordinary spends; three verified V4 unindexed trades remain at pool 3/6 and floor 90000; fresh pool accepts exact-fee CreateLaunch ({exact_fee}), latched pool rejects it"
    );
    Ok(())
}
