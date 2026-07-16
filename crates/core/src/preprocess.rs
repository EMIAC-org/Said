//! Audio conditioning for the speech paths — batch and realtime.
//!
//! Pipeline order (matches the design note):
//!   1. anti-aliased resample to 16 kHz — band-limited, no fold-back hiss
//!   2. DC removal + high-pass  — kills rumble / handling noise / mains hum
//!   3. RNNoise (`AIRNOTE_AUDIO_RNNOISE=0` disables) — stationary noise cleanup
//!   4. loudness normalization — consistent level into the model
//!   5. [whisper.cpp Silero VAD runs here, inside `whisper.full`]
//!
//! Two entry points share that chain:
//!
//! * [`condition_16k`] — **batch** (whole-utterance): steps 2, 3, 4. Used by the
//!   on-device Whisper/Oriserve and local-Nemotron paths; step 1 is
//!   [`resample_16k_hq`].
//! * [`StreamConditioner`] — **realtime** (per-chunk): steps 2 and 3 only, with
//!   every filter's state persisted across chunks so nothing clicks at a chunk
//!   boundary. Feeds the live cloud (Together Nemotron) socket on macOS and
//!   Windows alike.
//!
//! The realtime variant deliberately omits step 4: loudness normalization
//! derives one gain from the whole utterance, so applying it per-chunk would
//! pump the level between chunks. Cloud ASR models normalize internally, so the
//! win here is denoise, not level.
//!
//! The `said-recorder` streaming *resamplers* remain untouched — they run on the
//! audio thread where per-call cost matters; `StreamConditioner` runs after them
//! on the drain thread.
//!
//! The default path keeps RNNoise on for shipped builds, with an env opt-out for
//! debugging noisy-device regressions (`AIRNOTE_AUDIO_RNNOISE=0`, honoured by
//! both entry points).

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

// ── Realtime (streaming) conditioning ────────────────────────────────────────

/// 16 kHz samples per processing block. 160 @ 16 kHz = 10 ms = exactly one
/// RNNoise frame once upsampled ×3, so blocks map 1:1 onto frames with no
/// partial-frame padding mid-stream.
const BLOCK_16K: usize = nnnoiseless::DenoiseState::FRAME_SIZE / 3;

/// Streaming counterpart of [`condition_16k`] for the live cloud path.
///
/// Chunks arrive continuously (~100 ms each) and must be conditioned without
/// re-initialising any filter: a fresh biquad or denoiser per chunk rings and
/// clicks at every boundary. So each stage's state lives here for the whole
/// recording:
///
/// * `hp` — the 70 Hz high-pass, ticked sample-by-sample across chunks.
/// * `denoise` — one RNNoise state; it is frame-based by design and *expects*
///   to see a continuous stream (its noise estimate adapts over time).
/// * `lp48` — anti-alias low-pass before decimating 48 kHz → 16 kHz.
/// * `prev` / `pending` — carry the sub-block remainder and the interpolation
///   anchor, so upsampling is continuous across chunk boundaries too.
///
/// Input and output are both 16 kHz mono f32. Output lags input by at most one
/// block (10 ms) plus one sample; [`flush`](Self::flush) drains the remainder at
/// end of recording.
pub struct StreamConditioner {
    hp: Biquad,
    lp48: [Biquad; 2],
    /// `None` when RNNoise is disabled via `AIRNOTE_AUDIO_RNNOISE=0`; the
    /// high-pass still runs, matching the batch path's behaviour.
    denoise: Option<Box<nnnoiseless::DenoiseState<'static>>>,
    pending: Vec<f32>,
    /// Last input sample of the previous block — the left anchor for the next
    /// block's linear interpolation, so the ×3 upsample has no seam.
    prev: f32,
    primed: bool,
    /// RNNoise's first output frame carries fade-in artifacts; the batch path
    /// passes the original samples through instead, and so do we.
    first_frame: bool,
    frames: usize,
    vad_sum: f32,
}

impl Default for StreamConditioner {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamConditioner {
    pub fn new() -> Self {
        let fs48 = RNNOISE_RATE as f32;
        Self {
            hp: Biquad::high_pass(
                HPF_CUTOFF_HZ,
                TARGET_RATE as f32,
                std::f32::consts::FRAC_1_SQRT_2,
            ),
            lp48: [
                Biquad::low_pass(LPF_CUTOFF_HZ, fs48, BUTTER_4TH_Q[0]),
                Biquad::low_pass(LPF_CUTOFF_HZ, fs48, BUTTER_4TH_Q[1]),
            ],
            denoise: (!env_flag_disabled(RNNOISE_ENV))
                .then(|| nnnoiseless::DenoiseState::with_model(rnnoise_model())),
            pending: Vec::with_capacity(BLOCK_16K * 2),
            prev: 0.0,
            primed: false,
            first_frame: true,
            frames: 0,
            vad_sum: 0.0,
        }
    }

    /// True when RNNoise is active for this session (env opt-out not set).
    pub fn denoise_enabled(&self) -> bool {
        self.denoise.is_some()
    }

    /// Condition one chunk of 16 kHz mono audio. Returns the samples that are
    /// ready — whole blocks only; a sub-block remainder is held for the next
    /// call (or [`flush`](Self::flush)).
    pub fn push(&mut self, input: &[f32]) -> Vec<f32> {
        if input.is_empty() {
            return Vec::new();
        }
        let mut work = input.to_vec();
        self.hp.run(&mut work); // persistent state → continuous across chunks
        self.pending.extend_from_slice(&work);

        let ready = (self.pending.len() / BLOCK_16K) * BLOCK_16K;
        let mut out = Vec::with_capacity(ready);
        while self.pending.len() >= BLOCK_16K {
            let mut block = [0.0_f32; BLOCK_16K];
            block.copy_from_slice(&self.pending[..BLOCK_16K]);
            self.pending.drain(..BLOCK_16K);
            out.extend_from_slice(&self.process_block(&block));
        }
        out
    }

    /// Drain the final sub-block remainder at end of recording, zero-padded to a
    /// whole block and trimmed back so no samples are invented.
    pub fn flush(&mut self) -> Vec<f32> {
        if self.pending.is_empty() {
            return Vec::new();
        }
        let n = self.pending.len();
        let mut block = [0.0_f32; BLOCK_16K];
        block[..n].copy_from_slice(&self.pending);
        self.pending.clear();
        let mut out = self.process_block(&block).to_vec();
        out.truncate(n);
        out
    }

    /// Mean RNNoise speech probability across the session (diagnostics).
    pub fn mean_vad(&self) -> f32 {
        if self.frames == 0 {
            0.0
        } else {
            self.vad_sum / self.frames as f32
        }
    }

    pub fn frames(&self) -> usize {
        self.frames
    }

    /// One 10 ms block: ×3 upsample → RNNoise frame → anti-alias → ÷3 decimate.
    fn process_block(&mut self, block: &[f32; BLOCK_16K]) -> [f32; BLOCK_16K] {
        // The very first block has no predecessor; anchoring on its own first
        // sample makes the opening interval flat instead of a step up from 0.
        if !self.primed {
            self.prev = block[0];
            self.primed = true;
        }

        // ×3 linear upsample, anchored on the previous block's last sample so
        // the interpolation is seamless across the boundary. 160 intervals × 3
        // = exactly one 480-sample RNNoise frame.
        let mut up = [0.0_f32; nnnoiseless::DenoiseState::FRAME_SIZE];
        let mut a = self.prev;
        for (i, &b) in block.iter().enumerate() {
            let d = b - a;
            up[i * 3] = a;
            up[i * 3 + 1] = a + d / 3.0;
            up[i * 3 + 2] = a + 2.0 * d / 3.0;
            a = b;
        }
        self.prev = a;

        if let Some(state) = self.denoise.as_mut() {
            // RNNoise works in i16-scaled floats, like the batch path.
            let mut in_frame = [0.0_f32; nnnoiseless::DenoiseState::FRAME_SIZE];
            for (dst, src) in in_frame.iter_mut().zip(up.iter()) {
                *dst = (*src * RNNOISE_I16_SCALE).clamp(i16::MIN as f32, i16::MAX as f32);
            }
            let mut out_frame = [0.0_f32; nnnoiseless::DenoiseState::FRAME_SIZE];
            self.vad_sum += state.process_frame(&mut out_frame, &in_frame);
            self.frames += 1;

            let source = if self.first_frame { &in_frame } else { &out_frame };
            self.first_frame = false;
            for (dst, src) in up.iter_mut().zip(source.iter()) {
                *dst = (*src / RNNOISE_I16_SCALE).clamp(-1.0, 1.0);
            }
        }

        // Anti-alias before decimating back to 16 kHz (mirrors resample_16k_hq),
        // with the filter state persisted so blocks join cleanly.
        for bq in self.lp48.iter_mut() {
            bq.run(&mut up);
        }
        let mut out = [0.0_f32; BLOCK_16K];
        for (i, dst) in out.iter_mut().enumerate() {
            *dst = up[i * 3];
        }
        out
    }
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

    // ── StreamConditioner ────────────────────────────────────────────────────

    fn speech_like(n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let t = i as f32 / TARGET_RATE as f32;
                0.05 * (2.0 * PI * 220.0 * t).sin() + 0.015 * (2.0 * PI * 1_900.0 * t).sin()
            })
            .collect()
    }

    #[test]
    fn block_size_maps_exactly_onto_one_rnnoise_frame() {
        assert_eq!(BLOCK_16K * 3, nnnoiseless::DenoiseState::FRAME_SIZE);
        assert_eq!(BLOCK_16K, 160, "160 samples @16k == 10ms");
    }

    #[test]
    fn stream_conserves_sample_count_across_chunks() {
        let input = speech_like(16_000);
        let mut sc = StreamConditioner::new();
        let mut out = Vec::new();
        // Deliberately ragged chunk sizes — the recorder does not promise
        // block-aligned chunks.
        for chunk in [300_usize, 1_600, 77, 4_096, 1, 900]
            .iter()
            .cycle()
            .scan(0usize, |pos, &len| {
                if *pos >= input.len() {
                    return None;
                }
                let end = (*pos + len).min(input.len());
                let slice = &input[*pos..end];
                *pos = end;
                Some(slice)
            })
        {
            out.extend(sc.push(chunk));
        }
        out.extend(sc.flush());
        assert_eq!(out.len(), input.len(), "streaming must not drop or invent samples");
        assert!(out.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn chunking_does_not_change_the_result() {
        // The whole point of persistent state: how the stream is sliced must not
        // affect the output. One big push vs many small pushes must agree.
        let input = speech_like(8_000);
        let mut whole = StreamConditioner::new();
        let mut a = whole.push(&input);
        a.extend(whole.flush());

        let mut split = StreamConditioner::new();
        let mut b = Vec::new();
        for chunk in input.chunks(137) {
            b.extend(split.push(chunk));
        }
        b.extend(split.flush());

        assert_eq!(a.len(), b.len());
        for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
            assert!((x - y).abs() < 1.0e-6, "chunking changed sample {i}: {x} vs {y}");
        }
    }

    #[test]
    fn stream_has_no_discontinuity_at_block_boundaries() {
        // A click shows up as a sample-to-sample jump far larger than the
        // signal's own slope. Compare the worst step against a clean reference.
        let input = speech_like(16_000);
        let mut sc = StreamConditioner::new();
        let mut out = Vec::new();
        for chunk in input.chunks(1_600) {
            out.extend(sc.push(chunk));
        }
        out.extend(sc.flush());

        let max_step = out
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0_f32, f32::max);
        let input_max_step = input
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0_f32, f32::max);
        assert!(
            max_step < input_max_step * 4.0 + 0.01,
            "boundary click: max step {max_step} vs input {input_max_step}"
        );
    }

    #[test]
    fn stream_attenuates_low_frequency_rumble() {
        // 40 Hz rumble sits below the 70 Hz high-pass and must come out quieter.
        let rumble: Vec<f32> = (0..16_000)
            .map(|i| 0.3 * (2.0 * PI * 40.0 * i as f32 / TARGET_RATE as f32).sin())
            .collect();
        let mut sc = StreamConditioner::new();
        let mut out = Vec::new();
        for chunk in rumble.chunks(1_600) {
            out.extend(sc.push(chunk));
        }
        out.extend(sc.flush());
        let rms_in = (rumble.iter().map(|s| s * s).sum::<f32>() / rumble.len() as f32).sqrt();
        let rms_out = (out.iter().map(|s| s * s).sum::<f32>() / out.len() as f32).sqrt();
        assert!(rms_out < rms_in * 0.5, "rumble not attenuated: {rms_in} → {rms_out}");
    }

    #[test]
    fn stream_does_not_normalize_level() {
        // Realtime deliberately skips loudness normalization (it is buffer-global
        // and would pump per-chunk). A quiet signal must stay quiet.
        let quiet: Vec<f32> = (0..16_000)
            .map(|i| 0.01 * (2.0 * PI * 220.0 * i as f32 / TARGET_RATE as f32).sin())
            .collect();
        let mut sc = StreamConditioner::new();
        let mut out = Vec::new();
        for chunk in quiet.chunks(1_600) {
            out.extend(sc.push(chunk));
        }
        out.extend(sc.flush());
        let peak = out.iter().fold(0.0_f32, |m, &s| m.max(s.abs()));
        assert!(peak < 0.05, "streaming must not apply makeup gain, peak={peak}");
    }

    #[test]
    fn flush_on_empty_stream_is_a_noop() {
        let mut sc = StreamConditioner::new();
        assert!(sc.flush().is_empty());
        assert!(sc.push(&[]).is_empty());
    }
}
