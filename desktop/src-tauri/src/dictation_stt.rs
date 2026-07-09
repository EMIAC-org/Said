use said_core::transcript::{TranscriptMeta, TranscriptOrigin};

#[derive(Debug, Clone)]
pub struct LocalTranscript {
    pub transcript: String,
    pub meta: TranscriptMeta,
}

pub fn model_installed() -> bool {
    crate::meeting_engine::dictation_whisper_model_installed()
}

pub fn runtime_ready() -> bool {
    crate::meeting_engine::dictation_whisper_runtime_ready()
}

pub fn vad_installed() -> bool {
    crate::meeting_engine::silero_vad_model_installed()
}

pub async fn transcribe_wav_bytes(
    wav: &[u8],
    language: &str,
    prompt: Option<String>,
) -> Result<LocalTranscript, String> {
    if !model_installed() {
        return Err(
            "Local speech model is required. Download the on-device model in Settings.".into(),
        );
    }

    let started = std::time::Instant::now();
    let wav = wav.to_vec();
    let language = language.to_string();
    let local = tokio::task::spawn_blocking(move || {
        crate::local_asr::transcribe_wav_bytes(wav, language, prompt)
    })
    .await
    .map_err(|e| format!("local speech worker failed: {e}"))??;
    let duration_ms = local.total_ms.max(started.elapsed().as_millis() as u64);

    tracing::info!(
        total_ms = local.total_ms,
        queue_wait_ms = local.queue_wait_ms,
        load_ms = local.load_ms,
        inference_ms = local.inference_ms,
        model = %local.model,
        "[dictation_stt] local ASR complete"
    );

    let word_count = local.transcript.split_whitespace().count();
    Ok(LocalTranscript {
        transcript: local.transcript.clone(),
        meta: TranscriptMeta {
            enriched_transcript: local.transcript,
            confidence: 1.0,
            mean_word_confidence: 1.0,
            low_confidence_count: 0,
            word_count,
            languages: vec![local.language],
            model: local.model,
            duration_ms,
            origin: TranscriptOrigin::DictationLocal,
            ..TranscriptMeta::default()
        },
    })
}
