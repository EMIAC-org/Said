pub const ORISERVE_HINGLISH_MODEL: &str = "ggml-oriserve-hinglish-fp16.bin";
pub const LOCAL_SPEECH_PATH: &str = "local_batch";

pub fn telemetry_speech_model() -> String {
    crate::paths::active_dictation_model_path()
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(ORISERVE_HINGLISH_MODEL)
        .to_string()
}

pub fn telemetry_speech_path() -> &'static str {
    LOCAL_SPEECH_PATH
}

pub fn local_speech_ready(model_installed: bool) -> bool {
    model_installed
}
