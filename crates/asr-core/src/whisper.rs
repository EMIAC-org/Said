//! A warm, single-threaded whisper.cpp engine.
//!
//! One [`WhisperEngine`] owns at most one loaded model and is driven from a
//! single thread (the app's warm-worker thread, or the GPU worker process's
//! serve loop). It is deliberately *not* `Sync`: whisper.cpp contexts are not
//! safe to share across threads, so concurrency is achieved by running separate
//! engines (in-proc CPU + isolated GPU worker), never by sharing one.

use std::path::{Path, PathBuf};
use std::time::Instant;

use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperVadParams,
};

use crate::config::DictationLocalAsrConfig;
use crate::error::AsrError;

/// How the decoder trades accuracy for latency.
///
/// * `Accuracy` — beam-search 5 over the full padded 30 s encoder window.
///   This is whisper.cpp's CLI default and the app's historical behavior; it
///   remains the **macOS default** so quality there is untouched.
/// * `Realtime` — greedy decoding plus an encoder window sized to the actual
///   clip. Whisper always pays for a full 30 s window unless told otherwise, so
///   a 3 s dictation does 10× the needed encoder work; and beam-5 costs ~5× a
///   greedy pass (whisper.cpp PR #291 / discussion #297). Together this is a
///   ~4-8× latency cut on short clips — the **Windows/Linux default**, where
///   dictation was measured at 2-8 s per utterance.
///
/// Overridable both ways via `AIRNOTE_DICTATION_DECODE_PROFILE=accuracy|realtime`.
/// A degenerate-output guard retries `Realtime` results once with `Accuracy`
/// (the known failure mode of small audio windows is token repetition).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeProfile {
    Accuracy,
    Realtime,
}

impl DecodeProfile {
    fn resolve() -> Self {
        match std::env::var("AIRNOTE_DICTATION_DECODE_PROFILE")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "accuracy" => DecodeProfile::Accuracy,
            "realtime" | "fast" => DecodeProfile::Realtime,
            _ if cfg!(target_os = "macos") => DecodeProfile::Accuracy,
            _ => DecodeProfile::Realtime,
        }
    }
}

/// Clips longer than this keep the full 30 s window + multi-segment decoding —
/// the short-clip optimizations must never truncate a long dictation.
const SHORT_CLIP_MAX_SECS: f32 = 28.0;
/// Encoder-context floor. whisper.cpp's guidance (discussion #297) is not to go
/// below ~768 for general use: smaller windows trigger the decoder's
/// repetition glitch. Field-confirmed on the Hinglish fine-tune — a 6 s clip at
/// ctx=448 looped "hello hello hello…" for a minute on a weak CPU. 768 still
/// halves the encoder cost vs the full window.
const MIN_AUDIO_CTX: i32 = 768;
const FULL_AUDIO_CTX: i32 = 1500;

/// The compute device an engine loads its model onto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Device {
    /// CPU inference — always available, never aborts the process.
    Cpu,
    /// GPU inference on the given ggml device index. `Gpu(0)` is correct for
    /// macOS Metal; the Vulkan worker passes the index chosen by [`crate::probe`].
    Gpu(i32),
}

impl Device {
    fn use_gpu(self) -> bool {
        matches!(self, Device::Gpu(_))
    }
    fn index(self) -> i32 {
        match self {
            Device::Cpu => 0,
            Device::Gpu(i) => i,
        }
    }
}

/// A completed transcription plus the engine-local timing it can measure.
pub struct Transcribed {
    pub text: String,
    pub load_ms: u64,
    pub inference_ms: u64,
}

struct Loaded {
    model_path: PathBuf,
    ctx: WhisperContext,
}

/// A whisper.cpp engine pinned to one [`Device`], keeping its model warm across
/// requests.
pub struct WhisperEngine {
    device: Device,
    loaded: Option<Loaded>,
}

impl WhisperEngine {
    #[must_use]
    pub fn new(device: Device) -> Self {
        Self {
            device,
            loaded: None,
        }
    }

    #[must_use]
    pub fn device(&self) -> Device {
        self.device
    }

    #[must_use]
    pub fn is_loaded_for(&self, model: &Path) -> bool {
        self.loaded.as_ref().is_some_and(|l| l.model_path == model)
    }

    /// Whether any model is currently resident (used for idle-unload logging).
    #[must_use]
    pub fn is_loaded(&self) -> bool {
        self.loaded.is_some()
    }

    /// Drop the warm model, freeing device memory.
    pub fn unload(&mut self) {
        self.loaded = None;
    }

    /// Ensure the configured model is resident. Returns the load time in ms
    /// (0 when the already-warm model was reused).
    pub fn ensure_loaded(&mut self, cfg: &DictationLocalAsrConfig) -> Result<u64, AsrError> {
        if self.is_loaded_for(&cfg.model) {
            return Ok(0);
        }
        self.loaded = None;

        let model_path = cfg
            .model
            .to_str()
            .ok_or_else(|| AsrError::ModelLoad("model path is not valid UTF-8".into()))?;

        let mut params = WhisperContextParameters::default();
        params.use_gpu(self.device.use_gpu());
        params.gpu_device(self.device.index());

        let started = Instant::now();
        tracing::info!(
            model = %cfg.model.display(),
            device = ?self.device,
            "[asr-core] loading whisper model"
        );
        let ctx = WhisperContext::new_with_params(model_path, params)
            .map_err(|e| AsrError::ModelLoad(e.to_string()))?;
        let load_ms = started.elapsed().as_millis() as u64;
        tracing::info!(model = %cfg.model.display(), load_ms, "[asr-core] model loaded");

        self.loaded = Some(Loaded {
            model_path: cfg.model.clone(),
            ctx,
        });
        Ok(load_ms)
    }

    /// Transcribe already-prepared 16 kHz mono PCM (see [`crate::audio::prepare`]).
    ///
    /// Loads the model on first use, then decodes with the resolved
    /// [`DecodeProfile`]. A realtime result that looks like degenerate token
    /// repetition (the documented small-window failure mode) is retried once
    /// with the accuracy profile before being returned.
    ///
    /// Returns [`AsrError::NoTranscript`] if the result is empty after trimming;
    /// domain-level "low quality artifact" filtering is left to the caller.
    pub fn transcribe(
        &mut self,
        pcm16k: &[f32],
        cfg: &DictationLocalAsrConfig,
    ) -> Result<Transcribed, AsrError> {
        if pcm16k.is_empty() {
            return Err(AsrError::EmptyAudio);
        }
        let load_ms = self.ensure_loaded(cfg)?;

        let profile = DecodeProfile::resolve();
        let inference_started = Instant::now();
        let mut transcript = self.run_decode(pcm16k, cfg, profile, false)?;

        if profile == DecodeProfile::Realtime && looks_degenerate(&transcript) {
            // The repetition glitch comes from the shrunken encoder window, so
            // the retry only widens the window (still greedy) — a beam-5 retry
            // here cost ~50 s on a weak CPU in the field for no accuracy gain.
            tracing::warn!(
                preview = %said_core::text::truncate_utf8(&transcript, 80),
                "[asr-core] realtime decode looks degenerate; retrying with full window"
            );
            transcript = self.run_decode(pcm16k, cfg, profile, true)?;
        }

        Ok(Transcribed {
            text: transcript,
            load_ms,
            inference_ms: inference_started.elapsed().as_millis() as u64,
        })
    }

    /// One decode pass with the given profile. `full_window` forces the full
    /// 30 s encoder window (the degenerate-retry path). Returns the romanized,
    /// trimmed transcript or [`AsrError::NoTranscript`].
    fn run_decode(
        &self,
        pcm16k: &[f32],
        cfg: &DictationLocalAsrConfig,
        profile: DecodeProfile,
        full_window: bool,
    ) -> Result<String, AsrError> {
        let ctx = &self
            .loaded
            .as_ref()
            .ok_or_else(|| AsrError::Inference("model not loaded".into()))?
            .ctx;
        let mut state = ctx
            .create_state()
            .map_err(|e| AsrError::Inference(format!("create state: {e}")))?;

        let clip_secs = pcm16k.len() as f32 / crate::audio::WHISPER_SAMPLE_RATE as f32;
        let short_clip = clip_secs <= SHORT_CLIP_MAX_SECS;

        let mut params = match profile {
            DecodeProfile::Accuracy => FullParams::new(SamplingStrategy::BeamSearch {
                beam_size: 5,
                patience: -1.0,
            }),
            // Realtime quality scales with the engine's compute. A GPU decodes
            // beams nearly in parallel, so beam-2 buys back most of beam-5's
            // accuracy for a small cost (~2.4× cheaper than beam-5). On CPU:
            // greedy at temperature 0 with best_of=5 — whisper-cli's own
            // default — which costs nothing on the happy path and samples 5
            // candidates only when the quality gates trigger the temperature
            // fallback.
            DecodeProfile::Realtime if self.device.use_gpu() => {
                FullParams::new(SamplingStrategy::BeamSearch {
                    beam_size: 2,
                    patience: -1.0,
                })
            }
            DecodeProfile::Realtime => FullParams::new(SamplingStrategy::Greedy { best_of: 5 }),
        };
        params.set_language(Some(whisper_language(&cfg.language)));
        params.set_translate(false);
        params.set_n_max_text_ctx(cfg.max_context_tokens);
        params.set_no_timestamps(true);
        params.set_suppress_blank(true);
        params.set_suppress_nst(cfg.suppress_non_speech);
        params.set_temperature(0.0);
        params.set_temperature_inc(if cfg.no_fallback { 0.0 } else { 0.2 });
        if let Some(t) = cfg.entropy_threshold {
            params.set_entropy_thold(t);
        }
        if let Some(t) = cfg.logprob_threshold {
            params.set_logprob_thold(t);
        }
        if let Some(t) = cfg.no_speech_threshold {
            params.set_no_speech_thold(t);
        }
        if let Some(prompt) = cfg.prompt.as_deref().filter(|p| !p.contains('\0')) {
            params.set_initial_prompt(prompt);
        }
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_print_special(false);
        params.set_n_threads(decode_threads(profile));

        // Short-clip realtime path: size the encoder window to the audio instead
        // of paying the fixed 30 s cost, and decode as one segment. Long clips
        // keep the full window + multi-segment loop so nothing is truncated.
        let audio_ctx = if profile == DecodeProfile::Realtime && short_clip && !full_window {
            let ctx_frames = audio_ctx_for_secs(clip_secs);
            if ctx_frames < FULL_AUDIO_CTX {
                params.set_audio_ctx(ctx_frames);
                params.set_single_segment(true);
                params.set_no_context(true);
                ctx_frames
            } else {
                params.set_single_segment(false);
                0
            }
        } else {
            params.set_single_segment(false);
            0
        };

        if let Some(vad_model) = cfg.vad_model.as_deref().and_then(Path::to_str) {
            let mut vad = WhisperVadParams::new();
            vad.set_threshold(cfg.vad_threshold);
            vad.set_speech_pad(cfg.vad_speech_pad_ms);
            vad.set_min_silence_duration(cfg.vad_min_silence_ms);
            params.set_vad_model_path(Some(vad_model));
            params.set_vad_params(vad);
            params.enable_vad(true);
        }

        // Info-level on purpose: this is the one line that proves which decode
        // profile/window a dictation actually used (essential when diagnosing
        // "why was this slow" in the field).
        tracing::info!(
            ?profile,
            clip_secs,
            audio_ctx,
            threads = decode_threads(profile),
            "[asr-core] decode pass"
        );

        state
            .full(params, pcm16k)
            .map_err(|e| AsrError::Inference(e.to_string()))?;

        let n_segments = state.full_n_segments();
        let mut parts = Vec::new();
        for i in 0..n_segments {
            if let Some(segment) = state.get_segment(i) {
                if let Ok(text) = segment.to_str_lossy() {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        parts.push(trimmed.to_string());
                    }
                }
            }
        }

        let mut transcript = parts.join(" ");
        if cfg.romanize {
            transcript = said_core::script::enforce_roman_hinglish(&transcript);
        }
        let transcript = transcript.trim().to_string();
        if transcript.is_empty() {
            return Err(AsrError::NoTranscript);
        }
        Ok(transcript)
    }
}

/// Encoder context (in 20 ms frames, multiples of 64) sized to the clip:
/// `secs/30 · 1500 + 128` headroom, rounded up, floored at [`MIN_AUDIO_CTX`].
/// 1500 = the full 30 s window.
fn audio_ctx_for_secs(secs: f32) -> i32 {
    let raw = secs / 30.0 * FULL_AUDIO_CTX as f32 + 128.0;
    let rounded = ((raw / 64.0).ceil() as i32) * 64;
    rounded.clamp(MIN_AUDIO_CTX, FULL_AUDIO_CTX)
}

/// Detect the degenerate-repetition failure mode of small audio windows:
/// the decoder gets stuck emitting the same token(s) over and over.
fn looks_degenerate(text: &str) -> bool {
    let words: Vec<String> = text
        .split_whitespace()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|w| !w.is_empty())
        .collect();
    if words.len() < 8 {
        return false;
    }

    // 1. A run of >= 4 identical consecutive words.
    let mut run = 1usize;
    for pair in words.windows(2) {
        if pair[0] == pair[1] {
            run += 1;
            if run >= 4 {
                return true;
            }
        } else {
            run = 1;
        }
    }

    // 2. One word dominating a longer transcript (> 60%).
    let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for w in &words {
        *counts.entry(w.as_str()).or_default() += 1;
    }
    let max = counts.values().copied().max().unwrap_or(0);
    max * 10 > words.len() * 6
}

/// Normalize a language hint to the two codes we route between. Everything that
/// isn't clearly English is treated as Hindi (see the STT routing notes).
fn whisper_language(language: &str) -> &'static str {
    match language.trim().to_ascii_lowercase().as_str() {
        "en" | "english" => "en",
        _ => "hi",
    }
}

/// Thread count for CPU-side whisper work, overridable via
/// `AIRNOTE_DICTATION_WHISPER_THREADS`.
///
/// * `Accuracy` keeps the historical formula (logical cores clamped to 6) so
///   macOS behavior is byte-identical.
/// * `Realtime` uses **physical** cores: whisper is memory-bandwidth bound and
///   scheduling onto hyperthreads measurably slows it down (whisper.cpp #200) —
///   on a 4-core i5 the old clamp of 6 oversubscribed into SMT.
fn decode_threads(profile: DecodeProfile) -> i32 {
    if let Some(n) = std::env::var("AIRNOTE_DICTATION_WHISPER_THREADS")
        .ok()
        .and_then(|v| v.trim().parse::<i32>().ok())
        .filter(|v| *v > 0)
    {
        return n;
    }
    match profile {
        DecodeProfile::Accuracy => std::thread::available_parallelism()
            .map(|n| n.get().clamp(1, 6) as i32)
            .unwrap_or(4),
        DecodeProfile::Realtime => (num_cpus::get_physical().max(1) as i32).clamp(1, 8),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_ctx_scales_with_clip_and_stays_in_bounds() {
        // Short clips (≤ ~12 s) sit on the repetition-safe floor — half the
        // full window, never below it.
        assert_eq!(audio_ctx_for_secs(0.5), MIN_AUDIO_CTX);
        assert_eq!(audio_ctx_for_secs(2.0), MIN_AUDIO_CTX);
        assert_eq!(audio_ctx_for_secs(8.0), MIN_AUDIO_CTX);
        // Mid-length clips scale with audio, multiple of 64, still partial.
        let mid = audio_ctx_for_secs(16.0);
        let longer = audio_ctx_for_secs(20.0);
        assert!(mid > MIN_AUDIO_CTX && mid < FULL_AUDIO_CTX);
        assert!(longer > mid && longer < FULL_AUDIO_CTX);
        assert_eq!(mid % 64, 0);
        // 28 s+ → clamps to the full window (no risky shrinking).
        assert_eq!(audio_ctx_for_secs(28.0), FULL_AUDIO_CTX);
        assert_eq!(audio_ctx_for_secs(120.0), FULL_AUDIO_CTX);
    }

    #[test]
    fn degenerate_detector_flags_repetition_not_speech() {
        // Stuck decoder: same token repeated.
        assert!(looks_degenerate(
            "thank you thank thank thank thank thank thank you"
        ));
        assert!(looks_degenerate("ok ok ok ok ok ok ok ok ok"));
        // Normal dictation — including natural doubled words — is not flagged.
        assert!(!looks_degenerate(
            "main kal office ja raha hoon aur meeting hai"
        ));
        assert!(!looks_degenerate("hello hello kaise ho aap sab log theek"));
        // Too short to judge.
        assert!(!looks_degenerate("ok ok ok"));
    }
}
