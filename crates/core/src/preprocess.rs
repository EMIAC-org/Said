//! Optional audio conditioning for the on-device Whisper/Oriserve path.
//!
//! Always applied on the BATCH local-whisper path (whole-utterance). The
//! realtime streaming resamplers in `said-recorder` are deliberately left
//! untouched — they run per-chunk on the audio thread where persistent filter
//! state would click at chunk boundaries and per-call cost matters.
//!
//! Pipeline order (matches the design note):
//!   1. anti-aliased resample to 16 kHz — band-limited, no fold-back hiss
//!   2. DC removal + high-pass  — kills rumble / handling noise / mains hum
//!   3. RNNoise (`AIRNOTE_AUDIO_RNNOISE=0` disables) — stationary noise cleanup
//!   4. loudness normalization — consistent level into the model
//!   5. [whisper.cpp Silero VAD runs here, inside `whisper.full`]
//!
//! Steps 2, 3, and 4 are `condition_16k`; step 1 is `resample_16k_hq`. The
//! default path keeps RNNoise on for shipped builds, with an env opt-out for
//! debugging noisy-device regressions.

use std::f32::consts::PI;
use std::sync::OnceLock;
use std::time::Instant;

use tracing::debug;

/// Whisper's fixed input sample rate.
pub const TARGET_RATE: usize = 16_000;

// ── Tunables ─────────────────────────────────────────────────────────────────
/// High-pass cutoff. Below this is rumble / handling noise / mains hum, never
/// speech — the lowest male fundamentals sit around 85 Hz.
const HPF_CUTOFF_HZ: f32 = 70.0;
/// Anti-alias low-pass, just under the 8 kHz Nyquist of the 16 kHz target.
const LPF_CUTOFF_HZ: f32 = 7_600.0;
/// Loudness target (linear RMS). ~0.05 ≈ −26 dBFS RMS — a comfortable speech
/// level that still leaves headroom before the peak guard engages.
const TARGET_RMS: f32 = 0.05;
/// Never amplify by more than this, so a near-silent clip can't explode its
/// noise floor up into something the model mistakes for speech.
const MAX_GAIN: f32 = 8.0;
/// Below this input RMS the clip is treated as silence and left untouched.
const NOISE_FLOOR_RMS: f32 = 1.0e-3;
/// Post-gain peak ceiling — ~0.3 dB of headroom, so normalization never clips.
const PEAK_CEIL: f32 = 0.97;
/// Butterworth per-stage Q factors for a 4th-order (two-biquad) low-pass.
const BUTTER_4TH_Q: [f32; 2] = [0.541_20, 1.306_56];
/// RNNoise is enabled by default; this flag is an opt-out/debug switch.
const RNNOISE_ENV: &str = "AIRNOTE_AUDIO_RNNOISE";
/// RNNoise's fixed sample rate and frame shape.
const RNNOISE_RATE: usize = 48_000;
const RNNOISE_I16_SCALE: f32 = 32_768.0;

// ── Biquad ───────────────────────────────────────────────────────────────────

/// RBJ-cookbook biquad, transposed direct-form II, run in place. Coefficients
/// are pre-normalized by `a0` at construction.
#[derive(Clone, Copy)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    fn new(b0: f32, b1: f32, b2: f32, a0: f32, a1: f32, a2: f32) -> Self {
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    fn low_pass(fc: f32, fs: f32, q: f32) -> Self {
        let w0 = 2.0 * PI * fc / fs;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q);
        let b1 = 1.0 - cos;
        let b0 = b1 / 2.0;
        Self::new(b0, b1, b0, 1.0 + alpha, -2.0 * cos, 1.0 - alpha)
    }

    fn high_pass(fc: f32, fs: f32, q: f32) -> Self {
        let w0 = 2.0 * PI * fc / fs;
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q);
        let b0 = (1.0 + cos) / 2.0;
        Self::new(b0, -(1.0 + cos), b0, 1.0 + alpha, -2.0 * cos, 1.0 - alpha)
    }

    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }

    fn run(&mut self, buf: &mut [f32]) {
        for s in buf.iter_mut() {
            *s = self.tick(*s);
        }
    }
}

// ── Public DSP ───────────────────────────────────────────────────────────────

/// Band-limited resample to 16 kHz for a whole-utterance mono buffer.
///
/// Applies a 4th-order Butterworth low-pass just below the 8 kHz target Nyquist
/// before decimating, so high-frequency content (and mic hiss) can't fold back
/// into the speech band as it does with bare linear interpolation. Upsampling
/// adds no new aliasing, so it skips the filter and interpolates directly.
pub fn resample_16k_hq(input: &[f32], from_rate: usize) -> Vec<f32> {
    if input.is_empty() {
        return Vec::new();
    }
    if from_rate == TARGET_RATE {
        return input.to_vec();
    }
    let mut work = input.to_vec();
    if from_rate > TARGET_RATE {
        let fs = from_rate as f32;
        for q in BUTTER_4TH_Q {
            Biquad::low_pass(LPF_CUTOFF_HZ, fs, q).run(&mut work);
        }
    }
    linear_resample(&work, from_rate, TARGET_RATE)
}

/// In-place DC/high-pass + loudness normalization for a 16 kHz mono clip.
/// Returns the gain applied (1.0 = untouched). No-op on an empty buffer.
pub fn condition_16k(buf: &mut [f32]) -> f32 {
    if buf.is_empty() {
        return 1.0;
    }
    let t0 = Instant::now();

    // 1. DC removal + high-pass (2nd-order Butterworth @ ~70 Hz).
    Biquad::high_pass(
        HPF_CUTOFF_HZ,
        TARGET_RATE as f32,
        std::f32::consts::FRAC_1_SQRT_2,
    )
    .run(buf);

    // 3. Speech denoise. Keep it before normalization so
    // the final level is still controlled by one shared gain step.
    let rnnoise = maybe_denoise_rnnoise_16k(buf);

    // 4. Loudness normalization (silence-floor + peak guarded).
    let gain = normalize(buf);

    debug!(
        "[preprocess] conditioned {} samples: hp@{}Hz rnnoise={} gain×{:.2} in {}µs",
        buf.len(),
        HPF_CUTOFF_HZ as u32,
        rnnoise
            .map(|s| format!("{}frames/{:.2}vad/{}µs", s.frames, s.mean_vad, s.elapsed_us))
            .unwrap_or_else(|| "off".to_string()),
        gain,
        t0.elapsed().as_micros()
    );
    gain
}

#[derive(Clone, Copy, Debug)]
struct RnnoiseStats {
    frames: usize,
    mean_vad: f32,
    elapsed_us: u128,
}

fn maybe_denoise_rnnoise_16k(buf: &mut [f32]) -> Option<RnnoiseStats> {
    if env_flag_disabled(RNNOISE_ENV) {
        return None;
    }
    Some(denoise_rnnoise_16k(buf))
}

fn env_flag_disabled(name: &str) -> bool {
    std::env::var(name)
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off"
            )
        })
        .unwrap_or(false)
}

fn denoise_rnnoise_16k(buf: &mut [f32]) -> RnnoiseStats {
    let t0 = Instant::now();
    if buf.is_empty() {
        return RnnoiseStats {
            frames: 0,
            mean_vad: 0.0,
            elapsed_us: 0,
        };
    }

    let mut input_48k_i16 = linear_resample(buf, TARGET_RATE, RNNOISE_RATE);
    for sample in &mut input_48k_i16 {
        *sample = (*sample * RNNOISE_I16_SCALE).clamp(i16::MIN as f32, i16::MAX as f32);
    }

    let mut state = nnnoiseless::DenoiseState::with_model(rnnoise_model());
    let mut in_frame = [0.0_f32; nnnoiseless::DenoiseState::FRAME_SIZE];
    let mut out_frame = [0.0_f32; nnnoiseless::DenoiseState::FRAME_SIZE];
    let mut output_48k_i16 = Vec::with_capacity(input_48k_i16.len());
    let mut vad_sum = 0.0_f32;
    let mut frames = 0_usize;

    for (idx, chunk) in input_48k_i16
        .chunks(nnnoiseless::DenoiseState::FRAME_SIZE)
        .enumerate()
    {
        in_frame.fill(0.0);
        in_frame[..chunk.len()].copy_from_slice(chunk);
        let vad = state.process_frame(&mut out_frame, &in_frame);
        vad_sum += vad;
        frames += 1;

        // RNNoise's first output frame can contain fade-in artifacts. Preserve
        // the original first 10 ms instead of dropping or shifting audio.
        let source = if idx == 0 { &in_frame } else { &out_frame };
        output_48k_i16.extend(source[..chunk.len()].iter().copied());
    }

    for sample in &mut output_48k_i16 {
        *sample = (*sample / RNNOISE_I16_SCALE).clamp(-1.0, 1.0);
    }
    let output_16k = resample_16k_hq(&output_48k_i16, RNNOISE_RATE);
    for (dst, src) in buf.iter_mut().zip(output_16k.iter()) {
        *dst = *src;
    }
    if output_16k.len() < buf.len() {
        for sample in &mut buf[output_16k.len()..] {
            *sample = 0.0;
        }
    }

    RnnoiseStats {
        frames,
        mean_vad: if frames == 0 {
            0.0
        } else {
            vad_sum / frames as f32
        },
        elapsed_us: t0.elapsed().as_micros(),
    }
}

fn rnnoise_model() -> &'static nnnoiseless::RnnModel {
    static MODEL: OnceLock<nnnoiseless::RnnModel> = OnceLock::new();
    MODEL.get_or_init(nnnoiseless::RnnModel::default)
}

/// Scale `buf` toward `TARGET_RMS`. Leaves silence untouched, caps amplification
/// at `MAX_GAIN`, and never lets the loudest peak exceed `PEAK_CEIL`.
fn normalize(buf: &mut [f32]) -> f32 {
    let n = buf.len() as f32;
    let rms = (buf.iter().map(|s| s * s).sum::<f32>() / n).sqrt();
    if rms < NOISE_FLOOR_RMS {
        return 1.0; // silence / near-silent — don't amplify the noise floor
    }
    let mut gain = (TARGET_RMS / rms).min(MAX_GAIN);
    let peak = buf.iter().fold(0.0_f32, |m, &s| m.max(s.abs()));
    if peak > 0.0 {
        gain = gain.min(PEAK_CEIL / peak); // headroom guard — no clipping
    }
    if (gain - 1.0).abs() < 1.0e-3 {
        return 1.0;
    }
    for s in buf.iter_mut() {
        *s *= gain;
    }
    gain
}

/// Linear-interpolation resample. Identical math to the previous inline
/// resampler; kept here so `resample_16k_hq` can pre-filter then reuse it.
fn linear_resample(input: &[f32], from_rate: usize, to_rate: usize) -> Vec<f32> {
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
    fn resample_identity_when_already_16k() {
        let input = vec![0.1, 0.2, 0.3, 0.4];
        assert_eq!(resample_16k_hq(&input, TARGET_RATE), input);
    }

    #[test]
    fn resample_downsample_length() {
        let input: Vec<f32> = (0..48_000).map(|i| (i as f32 * 0.01).sin()).collect();
        let out = resample_16k_hq(&input, 48_000);
        assert!((out.len() as i64 - 16_000).abs() <= 2, "got {}", out.len());
    }

    #[test]
    fn anti_alias_attenuates_high_tone() {
        // A 12 kHz tone at 48 kHz would fold to 4 kHz under bare decimation. The
        // low-pass should crush it; RMS of the 16 kHz output stays small.
        let fs = 48_000.0_f32;
        let input: Vec<f32> = (0..48_000)
            .map(|i| (2.0 * PI * 12_000.0 * i as f32 / fs).sin())
            .collect();
        let out = resample_16k_hq(&input, 48_000);
        let rms = (out.iter().map(|s| s * s).sum::<f32>() / out.len() as f32).sqrt();
        assert!(rms < 0.15, "high tone leaked through: rms={rms}");
    }

    #[test]
    fn condition_preserves_length_and_boosts_quiet() {
        let mut buf: Vec<f32> = (0..16_000)
            .map(|i| 0.01 * (i as f32 * 0.05).sin())
            .collect();
        let len = buf.len();
        let gain = condition_16k(&mut buf);
        assert_eq!(buf.len(), len);
        assert!(gain > 1.0, "quiet clip should be amplified, got {gain}");
    }

    #[test]
    fn condition_leaves_silence_alone() {
        let mut buf = vec![0.0_f32; 8_000];
        assert_eq!(condition_16k(&mut buf), 1.0);
    }

    #[test]
    fn rnnoise_preserves_length_and_finite_samples() {
        let mut buf: Vec<f32> = (0..16_000)
            .map(|i| {
                let t = i as f32 / TARGET_RATE as f32;
                let speech = 0.05 * (2.0 * PI * 220.0 * t).sin();
                let noise = 0.015 * (2.0 * PI * 1_900.0 * t).sin();
                speech + noise
            })
            .collect();
        let len = buf.len();
        let stats = denoise_rnnoise_16k(&mut buf);
        assert_eq!(buf.len(), len);
        assert!(stats.frames > 0);
        assert!(stats.mean_vad.is_finite());
        assert!(buf.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn normalize_never_clips() {
        let mut buf: Vec<f32> = (0..16_000).map(|i| 0.9 * (i as f32 * 0.05).sin()).collect();
        condition_16k(&mut buf);
        let peak = buf.iter().fold(0.0_f32, |m, &s| m.max(s.abs()));
        assert!(peak <= PEAK_CEIL + 1.0e-4, "peak {peak} exceeded ceiling");
    }
}
