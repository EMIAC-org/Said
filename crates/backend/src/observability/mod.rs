//! Non-blocking observability pipeline for control-plane dictation history.

pub mod meeting_scanner;
pub mod outbox;
pub mod uploader;

pub use outbox::{
    DictationPatchPayload, DictationUpsertPayload, MeetingProviderUsagePayload,
    MeetingSessionPayload, RecordingObservabilityExtras, after_recording_insert,
    enqueue_dictation_patch, enqueue_dictation_upsert, should_enqueue,
};

pub fn observability_extras(client_run_id: Option<&str>) -> RecordingObservabilityExtras {
    RecordingObservabilityExtras {
        client_run_id: client_run_id.map(str::to_string),
        device_id: Some(said_core::paths::device_id()),
        platform: Some(std::env::consts::OS.to_string()),
        app_version: Some(env!("CARGO_PKG_VERSION").to_string()),
    }
}
