pub const ORISERVE_HINGLISH_MODEL: &str = "ggml-oriserve-hinglish-fp16.bin";

pub fn telemetry_speech_model() -> String {
    crate::paths::active_dictation_model_path()
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(ORISERVE_HINGLISH_MODEL)
        .to_string()
}
