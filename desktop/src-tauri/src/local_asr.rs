use std::{
    path::{Path, PathBuf},
    sync::{OnceLock, mpsc},
    thread,
    time::{Duration, Instant},
};

use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperVadParams,
};

const WHISPER_SAMPLE_RATE: usize = 16_000;
const DEFAULT_IDLE_UNLOAD_SECS: u64 = 600;
const DEFAULT_PREWARM_LANGUAGE: &str = "hinglish";

#[derive(Debug, Clone)]
pub struct LocalAsrOutput {
    pub transcript: String,
    pub model: String,
    pub language: String,
    pub total_ms: u64,
    pub load_ms: u64,
    pub inference_ms: u64,
    pub queue_wait_ms: u64,
}

struct Worker {
    tx: mpsc::Sender<Job>,
}

struct Job {
    queued_at: Instant,
    request: JobRequest,
    reply: Option<mpsc::Sender<Result<LocalAsrOutput, String>>>,
}

enum JobRequest {
    Prewarm { language_pref: String },
    Transcribe { wav: Vec<u8>, language_pref: String },
}

struct LoadedModel {
    model_path: PathBuf,
    ctx: WhisperContext,
}

static WORKER: OnceLock<Worker> = OnceLock::new();

pub fn prewarm_default_language() {
    if !env_bool("AIRNOTE_DICTATION_ASR_PREWARM", true) {
        return;
    }
    let job = Job {
        queued_at: Instant::now(),
        request: JobRequest::Prewarm {
            language_pref: DEFAULT_PREWARM_LANGUAGE.to_string(),
        },
        reply: None,
    };
    if let Err(e) = worker().tx.send(job) {
        tracing::warn!("[local_asr] failed to queue prewarm: {e}");
    }
}

pub fn transcribe_wav_bytes(wav: Vec<u8>, language_pref: String) -> Result<LocalAsrOutput, String> {
    let (tx, rx) = mpsc::channel();
    let job = Job {
        queued_at: Instant::now(),
        request: JobRequest::Transcribe { wav, language_pref },
        reply: Some(tx),
    };
    worker()
        .tx
        .send(job)
        .map_err(|e| format!("local speech worker is not available: {e}"))?;
    rx.recv()
        .map_err(|e| format!("local speech worker stopped: {e}"))?
}

fn worker() -> &'static Worker {
    WORKER.get_or_init(start_worker)
}

fn start_worker() -> Worker {
    let (tx, rx) = mpsc::channel();
    if let Err(e) = thread::Builder::new()
        .name("airnote-local-asr".to_string())
        .spawn(move || run_worker(rx))
    {
        tracing::error!("[local_asr] failed to start worker thread: {e}");
    }
    Worker { tx }
}

fn run_worker(rx: mpsc::Receiver<Job>) {
    let idle_timeout = idle_unload_timeout();
    let mut loaded: Option<LoadedModel> = None;

    loop {
        let job = if loaded.is_some() {
            match idle_timeout {
                Some(timeout) => match rx.recv_timeout(timeout) {
                    Ok(job) => job,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if let Some(model) = loaded.take() {
                            tracing::info!(
                                model = %model.model_path.display(),
                                idle_secs = timeout.as_secs(),
                                "[local_asr] unloaded warm model after idle timeout"
                            );
                        }
                        continue;
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                },
                None => match rx.recv() {
                    Ok(job) => job,
                    Err(_) => break,
                },
            }
        } else {
            match rx.recv() {
                Ok(job) => job,
                Err(_) => break,
            }
        };

        let queue_wait_ms = job.queued_at.elapsed().as_millis() as u64;
        match job.request {
            JobRequest::Prewarm { language_pref } => {
                if let Err(e) = handle_prewarm(&mut loaded, &language_pref, queue_wait_ms) {
                    tracing::warn!(error = %e, "[local_asr] prewarm failed");
                }
            }
            JobRequest::Transcribe { wav, language_pref } => {
                let result = handle_transcribe(&mut loaded, wav, &language_pref, queue_wait_ms);
                if let Some(reply) = job.reply {
                    let _ = reply.send(result);
                }
            }
        }
    }
}

fn handle_prewarm(
    loaded: &mut Option<LoadedModel>,
    language_pref: &str,
    queue_wait_ms: u64,
) -> Result<(), String> {
    let cfg = crate::meeting_engine::resolve_dictation_local_asr_config(language_pref)?;
    let load_ms = ensure_model_loaded(loaded, &cfg)?;
    tracing::info!(
        model = %model_label(&cfg.model),
        load_ms,
        queue_wait_ms,
        "[local_asr] warm model ready"
    );
    Ok(())
}

fn handle_transcribe(
    loaded: &mut Option<LoadedModel>,
    wav: Vec<u8>,
    language_pref: &str,
    queue_wait_ms: u64,
) -> Result<LocalAsrOutput, String> {
    let total_started = Instant::now();
    if wav.len() <= 44 {
        return Err("recording audio is empty".to_string());
    }

    let cfg = crate::meeting_engine::resolve_dictation_local_asr_config(language_pref)?;
    let mut audio = decode_wav_to_f32(&wav)?;
    if audio.is_empty() {
        return Err("recording audio is empty".to_string());
    }
    said_core::preprocess::condition_16k(&mut audio);

    let load_ms = ensure_model_loaded(loaded, &cfg)?;
    let model_label = model_label(&cfg.model);
    let inference_started = Instant::now();
    let transcript = {
        let ctx = &loaded
            .as_ref()
            .ok_or_else(|| "local speech model was not loaded".to_string())?
            .ctx;
        transcribe_pcm(ctx, &audio, &cfg)?
    };
    let inference_ms = inference_started.elapsed().as_millis() as u64;
    let total_ms = total_started.elapsed().as_millis() as u64;
    let audio_duration_s = audio.len() as f64 / WHISPER_SAMPLE_RATE as f64;
    tracing::info!(
        model = %model_label,
        language = %cfg.language,
        audio_s = audio_duration_s,
        queue_wait_ms,
        load_ms,
        inference_ms,
        total_ms,
        preview = ?said_core::text::truncate_utf8(&transcript, 120),
        "[local_asr] dictation transcribed"
    );

    Ok(LocalAsrOutput {
        transcript,
        model: model_label,
        language: cfg.language,
        total_ms,
        load_ms,
        inference_ms,
        queue_wait_ms,
    })
}

fn ensure_model_loaded(
    loaded: &mut Option<LoadedModel>,
    cfg: &crate::meeting_engine::DictationLocalAsrConfig,
) -> Result<u64, String> {
    if loaded
        .as_ref()
        .is_some_and(|model| model.model_path == cfg.model)
    {
        return Ok(0);
    }

    *loaded = None;
    let model_path = cfg
        .model
        .to_str()
        .ok_or_else(|| "local speech model path is not valid UTF-8".to_string())?;
    let started = Instant::now();
    tracing::info!(model = %cfg.model.display(), "[local_asr] loading model");
    let ctx = WhisperContext::new_with_params(model_path, WhisperContextParameters::default())
        .map_err(|e| format!("failed to load local speech model: {e}"))?;
    let load_ms = started.elapsed().as_millis() as u64;
    tracing::info!(
        model = %cfg.model.display(),
        load_ms,
        "[local_asr] model loaded"
    );
    *loaded = Some(LoadedModel {
        model_path: cfg.model.clone(),
        ctx,
    });
    Ok(load_ms)
}

fn transcribe_pcm(
    ctx: &WhisperContext,
    audio_f32: &[f32],
    cfg: &crate::meeting_engine::DictationLocalAsrConfig,
) -> Result<String, String> {
    let mut state = ctx
        .create_state()
        .map_err(|e| format!("failed to create local speech state: {e}"))?;

    let mut params = FullParams::new(SamplingStrategy::BeamSearch {
        beam_size: 5,
        patience: -1.0,
    });
    let language = whisper_language(&cfg.language);
    params.set_language(Some(language));
    params.set_translate(false);
    params.set_n_max_text_ctx(cfg.max_context_tokens);
    params.set_no_timestamps(true);
    params.set_single_segment(false);
    params.set_suppress_blank(true);
    params.set_suppress_nst(cfg.suppress_non_speech);
    params.set_temperature(0.0);
    params.set_temperature_inc(if cfg.no_fallback { 0.0 } else { 0.2 });
    if let Some(threshold) = cfg.entropy_threshold {
        params.set_entropy_thold(threshold);
    }
    if let Some(threshold) = cfg.logprob_threshold {
        params.set_logprob_thold(threshold);
    }
    if let Some(threshold) = cfg.no_speech_threshold {
        params.set_no_speech_thold(threshold);
    }
    if let Some(prompt) = cfg.prompt.as_deref().filter(|p| !p.contains('\0')) {
        params.set_initial_prompt(prompt);
    }
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params.set_print_special(false);
    params.set_n_threads(dictation_threads());

    if let Some(vad_model) = cfg.vad_model.as_deref() {
        if let Some(vad_model) = vad_model.to_str() {
            let mut vad_params = WhisperVadParams::new();
            vad_params.set_threshold(cfg.vad_threshold);
            vad_params.set_speech_pad(cfg.vad_speech_pad_ms);
            vad_params.set_min_silence_duration(cfg.vad_min_silence_ms);
            params.set_vad_model_path(Some(vad_model));
            params.set_vad_params(vad_params);
            params.enable_vad(true);
        }
    }

    state
        .full(params, audio_f32)
        .map_err(|e| format!("local speech inference failed: {e}"))?;

    let n_segments = state.full_n_segments();
    let mut text_parts = Vec::new();
    for i in 0..n_segments {
        if let Some(segment) = state.get_segment(i) {
            if let Ok(text) = segment.to_str_lossy() {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    text_parts.push(trimmed.to_string());
                }
            }
        }
    }

    let mut transcript = text_parts.join(" ");
    if cfg.romanize {
        transcript = said_core::script::enforce_roman_hinglish(&transcript);
    }
    let transcript = transcript.trim().to_string();
    if transcript.is_empty()
        || crate::meeting_engine::is_low_quality_transcript_artifact(&transcript)
    {
        return Err("local speech returned no usable transcript".to_string());
    }
    Ok(transcript)
}

fn decode_wav_to_f32(wav_data: &[u8]) -> Result<Vec<f32>, String> {
    if wav_data.len() < 44 {
        return Err("WAV data too short".into());
    }
    if &wav_data[0..4] != b"RIFF" || &wav_data[8..12] != b"WAVE" {
        return Err("not a valid WAV file".into());
    }

    let channels = u16::from_le_bytes([wav_data[22], wav_data[23]]) as usize;
    let sample_rate = u32::from_le_bytes([wav_data[24], wav_data[25], wav_data[26], wav_data[27]]);
    let bits_per_sample = u16::from_le_bytes([wav_data[34], wav_data[35]]);
    let data_offset = find_data_chunk(wav_data).ok_or("WAV: no data chunk found")?;
    let pcm_data = &wav_data[data_offset..];

    let samples_f32: Vec<f32> = match bits_per_sample {
        16 => pcm_data
            .chunks_exact(2)
            .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]) as f32 / 32768.0)
            .collect(),
        32 => pcm_data
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect(),
        _ => return Err(format!("unsupported WAV bit depth: {bits_per_sample}")),
    };

    let mono = if channels > 1 {
        samples_f32
            .chunks(channels)
            .map(|frame| frame.iter().sum::<f32>() / channels as f32)
            .collect()
    } else {
        samples_f32
    };

    if sample_rate != WHISPER_SAMPLE_RATE as u32 {
        tracing::warn!(
            sample_rate,
            expected = WHISPER_SAMPLE_RATE,
            "[local_asr] resampling WAV for whisper"
        );
        Ok(said_core::preprocess::resample_16k_hq(
            &mono,
            sample_rate as usize,
        ))
    } else {
        Ok(mono)
    }
}

fn find_data_chunk(wav: &[u8]) -> Option<usize> {
    let mut pos = 12;
    while pos + 8 <= wav.len() {
        let chunk_id = &wav[pos..pos + 4];
        let chunk_size =
            u32::from_le_bytes([wav[pos + 4], wav[pos + 5], wav[pos + 6], wav[pos + 7]]) as usize;
        if chunk_id == b"data" {
            return Some(pos + 8);
        }
        pos += 8 + chunk_size;
        if pos % 2 != 0 {
            pos += 1;
        }
    }
    None
}

fn model_label(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("local-whisper")
        .to_string()
}

fn whisper_language(language: &str) -> &'static str {
    match language.trim().to_ascii_lowercase().as_str() {
        "en" | "english" => "en",
        "hi" | "hindi" | "hinglish" | "" => "hi",
        _ => "hi",
    }
}

fn dictation_threads() -> i32 {
    std::env::var("AIRNOTE_DICTATION_WHISPER_THREADS")
        .ok()
        .and_then(|value| value.trim().parse::<i32>().ok())
        .filter(|value| *value > 0)
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get().clamp(1, 6) as i32)
                .unwrap_or(4)
        })
}

fn idle_unload_timeout() -> Option<Duration> {
    let secs = std::env::var("AIRNOTE_DICTATION_ASR_IDLE_SECS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_IDLE_UNLOAD_SECS);
    if secs == 0 {
        None
    } else {
        Some(Duration::from_secs(secs.max(30)))
    }
}

fn env_bool(name: &str, default: bool) -> bool {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .and_then(|value| match value.as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        })
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_wav_rejects_invalid() {
        assert!(decode_wav_to_f32(b"not a wav").is_err());
        assert!(decode_wav_to_f32(&[0; 10]).is_err());
    }
}
