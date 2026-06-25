#![cfg(feature = "local-stt")]
//! Local STT via whisper.cpp — offline, no API key required.
//!
//! Accepts 16 kHz mono PCM (WAV or raw f32) and returns a transcript
//! matching the same `TranscriptResult` interface as Deepgram.

use std::path::Path;
use std::sync::Mutex;

use once_cell::sync::OnceCell;
use tracing::{debug, info, warn};
use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperVadParams,
};

use super::deepgram::{LOW_CONFIDENCE_THRESHOLD, TranscriptResult};

static WHISPER_CTX: OnceCell<Mutex<WhisperContext>> = OnceCell::new();

const WHISPER_SAMPLE_RATE: usize = 16_000;

pub fn ensure_model_loaded(model_path: &str) -> Result<(), String> {
    WHISPER_CTX.get_or_try_init(|| {
        let path = Path::new(model_path);
        if !path.is_file() {
            return Err(format!("whisper model not found: {model_path}"));
        }
        info!("[whisper] loading model from {model_path}");
        let t0 = std::time::Instant::now();
        let ctx = WhisperContext::new_with_params(model_path, WhisperContextParameters::default())
            .map_err(|e| format!("failed to load whisper model: {e}"))?;
        info!("[whisper] model loaded in {}ms", t0.elapsed().as_millis());
        Ok(Mutex::new(ctx))
    })?;
    Ok(())
}

pub fn transcribe_pcm(audio_f32: &[f32], language: &str) -> Result<TranscriptResult, String> {
    let ctx_mutex = WHISPER_CTX
        .get()
        .ok_or("whisper model not loaded — call ensure_model_loaded first")?;
    let ctx = ctx_mutex.lock().unwrap_or_else(|e| {
        warn!("[whisper] recovering from poisoned lock");
        e.into_inner()
    });

    let mut state = ctx
        .create_state()
        .map_err(|e| format!("failed to create whisper state: {e}"))?;

    // Beam search + temperature fallback matches the accuracy of the PyTorch /
    // transformers reference decoder (greedy best_of=1 is the weakest config and
    // noticeably worse on hard audio). beam_size=5 is whisper.cpp's own default;
    // temperature_inc enables OpenAI-style fallback when a window decodes with
    // low confidence / high repetition. See whisper.cpp #1035.
    let mut params = FullParams::new(SamplingStrategy::BeamSearch {
        beam_size: 5,
        patience: -1.0,
    });

    let lang = match language {
        "en" => Some("en"),
        "hi" | "hindi" => Some("hi"),
        _ => Some("hi"),
    };
    params.set_language(lang);
    params.set_translate(false);
    params.set_no_timestamps(true);
    params.set_suppress_blank(true);
    params.set_suppress_nst(false);
    // Allow multi-segment decoding so utterances longer than a 30s window aren't
    // truncated to a single segment.
    params.set_single_segment(false);
    params.set_no_context(true);
    params.set_temperature(0.0);
    // Fallback ladder (0.0 → 0.2 → 0.4 …) when entropy/logprob thresholds trip.
    params.set_temperature_inc(0.2);
    params.set_entropy_thold(2.4);
    params.set_logprob_thold(-1.0);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params.set_print_special(false);
    params.set_n_threads(4);

    // Silero VAD gate: when the model is present, whisper.cpp runs voice-activity
    // detection first and only transcribes detected speech. This kills the
    // silence/noise hallucination seen on longer dictations (the "phantom"
    // fluent text on pauses). Best-effort — skipped if the model isn't installed.
    let vad_path = said_core::paths::silero_vad_model_path();
    if vad_path.is_file() {
        if let Some(p) = vad_path.to_str() {
            params.set_vad_model_path(Some(p));
            params.set_vad_params(WhisperVadParams::new());
            params.enable_vad(true);
            debug!("[whisper] Silero VAD enabled ({p})");
        }
    } else {
        debug!("[whisper] Silero VAD model absent — running without VAD gate");
    }

    let t0 = std::time::Instant::now();
    state
        .full(params, audio_f32)
        .map_err(|e| format!("whisper inference failed: {e}"))?;
    let inference_ms = t0.elapsed().as_millis();

    let n_segments = state.full_n_segments();

    let mut text_parts = Vec::new();
    for i in 0..n_segments {
        if let Some(seg) = state.get_segment(i) {
            if let Ok(text) = seg.to_str_lossy() {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    text_parts.push(trimmed.to_string());
                }
            }
        }
    }
    let transcript = text_parts.join(" ");

    if transcript.is_empty() {
        return Err("whisper returned empty transcript".into());
    }

    let audio_duration_s = audio_f32.len() as f64 / WHISPER_SAMPLE_RATE as f64;
    let preview = truncate_utf8(&transcript, 120);
    info!(
        "[whisper] {:.1}s audio → {}ms inference, {} segment(s): {:?}",
        audio_duration_s, inference_ms, n_segments, preview
    );

    let word_count = transcript.split_whitespace().count();

    Ok(TranscriptResult {
        transcript: transcript.clone(),
        enriched_transcript: transcript,
        confidence: 0.80,
        uncertain_count: 0,
        mean_word_confidence: 0.80,
        word_count,
        languages: vec![lang.unwrap_or("hi").to_string()],
        stt_mode: format!("whisper_local:{}", lang.unwrap_or("hi")),
    })
}

pub fn transcribe_wav(wav_data: &[u8], language: &str) -> Result<TranscriptResult, String> {
    let audio_f32 = decode_wav_to_f32(wav_data)?;
    transcribe_pcm(&audio_f32, language)
}

fn truncate_utf8(s: &str, max_bytes: usize) -> &str {
    said_core::text::truncate_utf8(s, max_bytes)
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

    debug!(
        "[whisper] WAV: {}Hz {}ch {}bit",
        sample_rate, channels, bits_per_sample
    );

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
        warn!(
            "[whisper] WAV sample rate {}Hz != expected {}Hz — resampling",
            sample_rate, WHISPER_SAMPLE_RATE
        );
        Ok(resample(&mono, sample_rate as usize, WHISPER_SAMPLE_RATE))
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

fn resample(input: &[f32], from_rate: usize, to_rate: usize) -> Vec<f32> {
    if from_rate == to_rate || input.is_empty() {
        return input.to_vec();
    }
    let ratio = from_rate as f64 / to_rate as f64;
    let out_len = (input.len() as f64 / ratio).ceil() as usize;
    (0..out_len)
        .map(|i| {
            let src = i as f64 * ratio;
            let idx = src as usize;
            let frac = (src - idx as f64) as f32;
            let a = input[idx.min(input.len() - 1)];
            let b = input[(idx + 1).min(input.len() - 1)];
            a + frac * (b - a)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_wav_rejects_invalid() {
        assert!(decode_wav_to_f32(b"not a wav").is_err());
        assert!(decode_wav_to_f32(&[0; 10]).is_err());
    }

    #[test]
    fn resample_identity() {
        let input = vec![0.1, 0.2, 0.3, 0.4];
        let out = resample(&input, 16000, 16000);
        assert_eq!(out, input);
    }

    #[test]
    fn resample_downsample() {
        let input: Vec<f32> = (0..32000).map(|i| (i as f32 / 32000.0).sin()).collect();
        let out = resample(&input, 32000, 16000);
        assert!((out.len() as f64 - 16000.0).abs() < 2.0);
    }
}
