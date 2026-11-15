use noid_chain::consensus::forks::{ForkSchedule, V2Activation};
use noid_chain::history_step::HistoryStepTerminalMetadata;
use serde_json::json;

pub(crate) fn run() -> Result<(), Box<dyn std::error::Error>> {
    let schedule = ForkSchedule::new(Some(5), V2Activation::new(10, 30)).unwrap();
    let native = HistoryStepTerminalMetadata::new_with_protocol_schedule(
        24,
        [0x51; 32],
        2,
        schedule,
        Some(24),
    )?;
    let bytes = native.encode_prefix_with_schedule(schedule);
    let active = HistoryStepTerminalMetadata::decode_prefix_with_protocol_schedule(
        &bytes,
        schedule,
        Some(24),
    );
    let pre_activation = HistoryStepTerminalMetadata::decode_prefix_with_protocol_schedule(
        &bytes,
        schedule,
        Some(25),
    );
    let v2_only =
        HistoryStepTerminalMetadata::decode_prefix_with_protocol_schedule(&bytes, schedule, None);
    let too_early = HistoryStepTerminalMetadata::new_with_protocol_schedule(
        23,
        [0x51; 32],
        2,
        schedule,
        Some(24),
    );
    let mut downgraded_v5 = bytes;
    downgraded_v5[0] = 5;
    let downgraded_v5_result = HistoryStepTerminalMetadata::decode_prefix_with_protocol_schedule(
        &downgraded_v5,
        schedule,
        Some(24),
    );
    let mut downgraded_v4 = bytes;
    downgraded_v4[0] = 4;
    let downgraded_v4_result = HistoryStepTerminalMetadata::decode_prefix_with_protocol_schedule(
        &downgraded_v4,
        schedule,
        Some(24),
    );
    let other_v2_schedule = ForkSchedule::new(Some(3), V2Activation::new(8, 45)).unwrap();
    let other_schedule_metadata = HistoryStepTerminalMetadata::decode_prefix_with_protocol_schedule(
        &bytes,
        other_v2_schedule,
        Some(24),
    );

    assert!(active.is_ok());
    assert!(pre_activation.is_err());
    assert!(v2_only.is_err());
    assert!(too_early.is_err());
    assert!(downgraded_v5_result.is_err());
    assert!(downgraded_v4_result.is_err());
    assert!(other_schedule_metadata.is_ok());

    let output = json!({
        "check": "history-step-terminal-metadata-only",
        "wire_version": bytes[0],
        "height": native.terminal_height(),
        "class_id": native.class_id(),
        "native_at_activation": active.is_ok(),
        "native_before_activation": pre_activation.is_ok(),
        "native_without_v2_1_activation": v2_only.is_ok(),
        "native_one_height_early_constructor": too_early.is_ok(),
        "downgraded_v5_accepted": downgraded_v5_result.is_ok(),
        "downgraded_v4_accepted": downgraded_v4_result.is_ok(),
        "same_metadata_accepted_under_other_v2_schedule": other_schedule_metadata.is_ok(),
        "scope": "metadata admission only; a full terminal still requires its exact proof bank and verified origin"
    });
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}
