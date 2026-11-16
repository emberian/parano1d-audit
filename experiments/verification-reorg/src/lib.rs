//! Release-mode regression for the raw segmented-state shrink used by reorg.

#[cfg(test)]
mod tests {
    use noid_chain::fri_state::SlotValue;
    use noid_chain::state::ChainState;
    use noid_core::Block128;
    use std::panic::{catch_unwind, AssertUnwindSafe};

    fn loaded_boundary() -> ChainState {
        let slot = SlotValue::from_parts(10, 1, Block128(3), Block128(5));
        let mut state = ChainState::from_sparse_utxos(24, &[(7, slot)], 1)
            .expect("valid loaded sparse state");
        state.try_state_root().expect("exact root");
        state.state.clear_dirty(); // model a completed durable commit
        state
    }

    #[test]
    fn evicted_metadata_panics_on_reorg_shrink_while_loaded_control_succeeds() {
        let mut loaded = loaded_boundary();
        let original_root = loaded.cached_state_root();

        // Control: the same geometry with resident columns can recompute its
        // raw FRI commitment and shrink to the parent slot depth.
        loaded.expand_one();
        loaded.state.shrink_to_log_slots(24).expect("loaded shrink");
        assert_eq!(loaded.try_state_root().expect("control exact root"), original_root);

        // Production's metadata snapshot representation has authenticated
        // exact roots, but its raw segment commitment is deliberately dirty
        // and its live columns are evicted.
        let mut compact = loaded_boundary()
            .durable_metadata_clone()
            .expect("durable metadata snapshot");
        assert!(compact.state.is_evicted(0));
        compact.expand_one();
        assert_eq!(compact.state.log_slots(), 25);

        // MdbxChainContext::apply_reorg_mdbx_with_objects calls this same
        // shrink_to_log_slots after reverting an expansion block. Its flush
        // asserts on the evicted dirty segment even with debug checks off.
        let outcome = catch_unwind(AssertUnwindSafe(|| compact.state.shrink_to_log_slots(24)));
        assert!(outcome.is_err(), "evicted metadata must trigger the release assertion");
    }
}
