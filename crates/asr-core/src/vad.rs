//! Provider-independent Silero VAD helpers for conditioned 16 kHz PCM.
//!
//! Whisper can run this model internally. Other local engines (for example
//! Nemotron) need the same gate applied before their WAV is handed over.

use std::path::Path;

use whisper_rs::{WhisperVadContext, WhisperVadContextParams, WhisperVadParams};

use crate::audio::WHISPER_SAMPLE_RATE;
use crate::error::AsrError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VadMaskStats {
    pub speech_samples: usize,
    pub total_samples: usize,
}

/// Silence every sample outside a Silero speech segment while preserving the
/// original sample positions and length. Preserved timing matters for meeting
/// captions; zeroing still prevents background sound from reaching an ASR model.
pub fn mask_non_speech_16k(
    samples: &mut [f32],
    model_path: &Path,
    threshold: f32,
    speech_pad_ms: i32,
    min_silence_ms: i32,
) -> Result<VadMaskStats, AsrError> {
    if samples.is_empty() {
        return Ok(VadMaskStats {
            speech_samples: 0,
            total_samples: 0,
        });
    }
    let path = model_path
        .to_str()
        .ok_or_else(|| AsrError::ModelLoad("Silero VAD model path is not UTF-8".to_string()))?;
    let mut context = WhisperVadContext::new(path, WhisperVadContextParams::new())
        .map_err(|error| AsrError::ModelLoad(format!("could not load Silero VAD: {error}")))?;
    let mut params = WhisperVadParams::new();
    params.set_threshold(threshold.clamp(0.0, 1.0));
    params.set_speech_pad(speech_pad_ms.max(0));
    params.set_min_silence_duration(min_silence_ms.max(0));
    let segments = context
        .segments_from_samples(params, samples)
        .map_err(|error| AsrError::Inference(format!("Silero VAD failed: {error}")))?;

    let mut masked = vec![0.0_f32; samples.len()];
    let mut speech_samples = 0_usize;
    for segment in segments {
        let start = timestamp_to_sample(segment.start, samples.len());
        let end = timestamp_to_sample(segment.end, samples.len()).max(start);
        speech_samples = speech_samples.saturating_add(end.saturating_sub(start));
        masked[start..end].copy_from_slice(&samples[start..end]);
    }
    samples.copy_from_slice(&masked);
    Ok(VadMaskStats {
        speech_samples,
        total_samples: samples.len(),
    })
}

fn timestamp_to_sample(timestamp_centiseconds: f32, max: usize) -> usize {
    // whisper.cpp reports VAD boundaries in centiseconds (10 ms).
    ((timestamp_centiseconds.max(0.0) * WHISPER_SAMPLE_RATE as f32 / 100.0).round() as usize)
        .min(max)
}

#[cfg(test)]
mod tests {
    use super::timestamp_to_sample;

    #[test]
    fn converts_centisecond_timestamps_to_16k_samples() {
        assert_eq!(timestamp_to_sample(0.0, 16_000), 0);
        assert_eq!(timestamp_to_sample(100.0, 16_000), 16_000);
        assert_eq!(timestamp_to_sample(150.0, 16_000), 16_000);
    }
}
