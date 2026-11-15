use noid_chain::consensus::{forks::ACTIVE_SCHEDULE, network::NetworkConfig, params};
use noid_p2p::network_profile::NetworkProfile;
use serde_json::json;

pub(crate) fn run() -> Result<(), Box<dyn std::error::Error>> {
    let legacy_bank = [0x42; 32];
    let profile = NetworkProfile::for_proof_bank(legacy_bank);
    assert!(profile.is_for_proof_bank(legacy_bank));
    assert!(!profile.is_for_proof_bank([0x43; 32]));

    let output = json!({
        "check": "network-profile",
        "profile_id": crate::hex(&profile.profile_id),
        "genesis_hash": crate::hex(&profile.genesis_hash),
        "history_proof_bank_id": crate::hex(&profile.history_proof_bank_id),
        "wire_version": profile.wire_version,
        "history_terminal_version": profile.history_terminal_version,
        "history_class_count": profile.history_class_count,
        "p2p_protocol_id": NetworkConfig::mainnet().p2p_protocol_id,
        "v1_1_activation": ACTIVE_SCHEDULE.v1_1_height(),
        "v2_activation": ACTIVE_SCHEDULE.v2().map(|v| v.height()),
        "v2_1_activation": params::V2_1_ACTIVATION_HEIGHT,
        "owner_authorization_activation": params::OWNER_AUTHORIZATION_ACTIVATION_HEIGHT,
        "owner_version_at_10": params::owner_authorization_version(10) as u8,
        "owner_version_at_24": params::owner_authorization_version(24) as u8,
        "terminal_version_at_24": noid_chain::history_step::history_step_terminal_wire_version(24),
        "terminal_class_count_at_24": noid_chain::history_step::history_step_class_count(24),
        "native_chain_id": crate::hex(&noid_chain::state::native_assets::native_chain_id()),
        "selected_legacy_bank_accepted": profile.is_for_proof_bank(legacy_bank),
        "different_legacy_bank_accepted": profile.is_for_proof_bank([0x43; 32]),
    });
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}
