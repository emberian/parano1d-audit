//! Bounded template-allocator conformance checks against pinned Parano1d code.

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use noid_chain::block_header::BlockHeader;
    use noid_chain::consensus::fees::fee_breakdown;
    use noid_chain::consensus::pow::block_id;
    use noid_chain::consensus::template::{build_node_owned_block_template, TemplateBuildError};
    use noid_chain::fri_state::SlotValue;
    use noid_chain::state::ChainState;
    use noid_poseidon2b::primitives::Address;
    use noid_tx::{
        output_bitmap_bit, Transaction, TxBody, TxInput, TxOutput, PAGED_SPEND_END_BIT,
        PAGED_SPEND_START_BIT, TX_INPUTS, TX_OUTPUTS,
    };

    fn parent(state: &ChainState) -> BlockHeader {
        BlockHeader {
            prev_block_hash: [0; 32],
            state_root: state.cached_state_root(),
            tx_root: [0; 32],
            timestamp: 0,
            height: 0,
            miner_address: Address([0; 32]),
            nonce: 0,
            difficulty_target: [0xff; 32],
            log_slots: state.state.log_slots() as u32,
            active_slot_count: state.active_slot_count,
            alloc_counter: state.alloc_counter,
        }
    }

    fn two_hole_spend(parent: &BlockHeader, owner: Address) -> Transaction {
        let fee = fee_breakdown(1, 2, parent.active_slot_count, parent.log_slots).required_total;
        let mut inputs = [TxInput::dummy(); TX_INPUTS];
        inputs[0] = TxInput {
            slot_index: 1,
            amount: 1_000_000,
            creation_id: 1,
        };
        let mut outputs = [TxOutput::dummy(); TX_OUTPUTS];
        outputs[0] = TxOutput {
            slot_index: 65_534,
            amount: 400_000,
            owner,
        };
        outputs[1] = TxOutput {
            slot_index: 65_535,
            amount: 600_000 - fee,
            owner,
        };
        Transaction::new(TxBody {
            epoch_anchor: block_id(parent),
            fee,
            input_owner: owner,
            inputs,
            outputs,
            validity_bitmap: 1
                | output_bitmap_bit(0)
                | output_bitmap_bit(1)
                | PAGED_SPEND_START_BIT
                | PAGED_SPEND_END_BIT,
            is_coinbase: false,
        })
    }

    #[test]
    fn coinbase_requires_a_hydrated_unreserved_segment() {
        let owner = Address([7; 32]);
        let leaf = SlotValue::with_owner_fields(1_000_000, 1, owner.as_fields());
        // Two populated segments: segment 0 has exactly two holes, and segment 1
        // has abundant free slots. The selected payment claims both holes.
        let mut live: Vec<(u32, SlotValue)> = (0..65_534).map(|i| (i, leaf)).collect();
        live.push((65_536, leaf));
        let loaded = ChainState::from_sparse_utxos(17, &live, 1).unwrap();
        let parent = parent(&loaded);
        let payment = two_hole_spend(&parent, owner);

        let mut partially_loaded = loaded.clone();
        partially_loaded.state.clear_dirty();
        partially_loaded.state.evict_segment(1);
        assert!(partially_loaded.state.is_evicted(1));
        assert_eq!(partially_loaded.active_slot_count, loaded.active_slot_count);
        assert_eq!(partially_loaded.cached_state_root(), loaded.cached_state_root());

        // This is the same precheck used by TemplateChainSnapshot before it
        // knows the selected transaction's reserved output slots.
        let advertised = partially_loaded
            .state
            .empty_slot_hints_in_populated_segments(0, 1, &HashSet::new());
        assert_eq!(advertised.len(), 1);
        assert!([65_534, 65_535].contains(&advertised[0]));
        let reserved_outputs = HashSet::from([65_534, 65_535]);
        assert!(partially_loaded
            .state
            .empty_slot_hints_in_populated_segments(0, 1, &reserved_outputs)
            .is_empty());

        let blocked = build_node_owned_block_template(
            &parent,
            &partially_loaded,
            &[],
            vec![payment.clone()],
            Address([9; 32]),
            1,
            [0xff; 32],
        );
        assert!(matches!(blocked, Err(TemplateBuildError::NoCoinbaseSlot)));

        let control = build_node_owned_block_template(
            &parent,
            &loaded,
            &[],
            vec![payment],
            Address([9; 32]),
            1,
            [0xff; 32],
        );
        assert!(control.is_ok(), "hydrated control failed: {:?}", control.err());
    }
}
