//! WAV → conditioned 16 kHz mono f32 PCM.
//!
//! Done once on the app side; the resulting `Vec<f32>` is what both the
//! in-process engine and the GPU worker consume, so decoding never happens
//! twice and the worker IPC payload is compact (~512 KB for 8 s).

use crate::error::AsrError;

/// whisper.cpp's fixed input rate.
pub const WHISPER_SAMPLE_RATE: usize = 16_000;

/// Decode a WAV byte buffer to conditioned 16 kHz mono f32 samples.
///
/// Steps: validate RIFF/WAVE → decode 16- or 32-bit PCM → downmix to mono →
/// resample to 16 kHz if needed → [`said_core::preprocess::condition_16k`].
///
/// For whisper-family models only. Conformer/TDT models are trained on
/// unprocessed capture and decode poorly through this chain — they use
/// [`decode_16k`] instead.
pub fn prepare(wav: &[u8]) -> Result<Vec<f32>, AsrError> {
    let mut audio = decode_16k(wav)?;
    said_core::preprocess::condition_16k(&mut audio);
    Ok(audio)
}

/// Decode a WAV byte buffer to plain 16 kHz mono f32 samples, with no spectral
/// conditioning: validate RIFF/WAVE → decode → downmix → resample.
///
/// This is what the model sees for every non-whisper local engine. The
/// high-pass, RNNoise and loudness-normalization stages in
/// [`said_core::preprocess::condition_16k`] were tuned for whisper's mel
/// front-end; conformer/TDT models expect raw capture and can decode to nothing
/// through them.
pub fn decode_16k(wav: &[u8]) -> Result<Vec<f32>, AsrError> {
    if wav.len() <= 44 {
        return Err(AsrError::EmptyAudio);
    }
    let audio = decode_to_mono_16k(wav)?;
    if audio.is_empty() {
        return Err(AsrError::EmptyAudio);
    }
    Ok(audio)
}

fn decode_to_mono_16k(wav_data: &[u8]) -> Result<Vec<f32>, AsrError> {
    if wav_data.len() < 44 {
        return Err(AsrError::BadWav("WAV data too short".into()));
    }
    if &wav_data[0..4] != b"RIFF" || &wav_data[8..12] != b"WAVE" {
        return Err(AsrError::BadWav("not a valid WAV file".into()));
    }

    let channels = u16::from_le_bytes([wav_data[22], wav_data[23]]) as usize;
    let sample_rate = u32::from_le_bytes([wav_data[24], wav_data[25], wav_data[26], wav_data[27]]);
    let bits_per_sample = u16::from_le_bytes([wav_data[34], wav_data[35]]);
    let data_offset =
        find_data_chunk(wav_data).ok_or_else(|| AsrError::BadWav("no data chunk found".into()))?;
    let pcm_data = &wav_data[data_offset..];

    let samples_f32: Vec<f32> = match bits_per_sample {
        16 => pcm_data
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
            .collect(),
        32 => pcm_data
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect(),
        other => {
            return Err(AsrError::BadWav(format!(
                "unsupported WAV bit depth: {other}"
            )));
        }
    };

    let channels = channels.max(1);
    let mono: Vec<f32> = if channels > 1 {
        samples_f32
            .chunks(channels)
            .map(|frame| frame.iter().sum::<f32>() / channels as f32)
            .collect()
    } else {
        samples_f32
    };

    if sample_rate as usize != WHISPER_SAMPLE_RATE {
        tracing::warn!(
            sample_rate,
            expected = WHISPER_SAMPLE_RATE,
            "[asr-core] resampling WAV for whisper"
        );
        Ok(said_core::preprocess::resample_16k_hq(
            &mono,
            sample_rate as usize,
        ))
    } else {
        Ok(mono)
    }
}

/// Walk the RIFF chunk list to find the `data` chunk offset (handles WAVs with
/// `LIST`/`fact` chunks before `data`).
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid() {
        assert!(matches!(prepare(b"not a wav"), Err(AsrError::EmptyAudio)));
        assert!(matches!(prepare(&[0u8; 10]), Err(AsrError::EmptyAudio)));
    }
}
