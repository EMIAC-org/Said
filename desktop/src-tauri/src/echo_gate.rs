use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

const REFERENCE_BUFFER_SAMPLES: usize = 16_000 * 2;
const MIC_ACTIVE_RMS: f32 = 0.010;
const SPEAKER_ACTIVE_RMS: f32 = 0.006;
const ECHO_CORRELATION_THRESHOLD: f32 = 0.42;
const MAX_ECHO_LAG_SAMPLES: usize = 4_000; // 250 ms at 16 kHz
const LAG_STEP_SAMPLES: usize = 160; // 10 ms at 16 kHz

#[derive(Clone, Debug, Default)]
pub struct EchoGateStatus {
    pub speaker_reference_available: bool,
    pub echo_gate_active: bool,
    pub local_speech_active: bool,
    pub last_gate_reason: String,
}

#[derive(Clone, Debug)]
pub struct EchoDecision {
    pub allow: bool,
    pub mic_rms: f32,
    pub speaker_rms: f32,
    pub correlation: f32,
    pub reason: &'static str,
}

#[derive(Default)]
pub struct EchoGateShared {
    active: AtomicBool,
    reference_available: AtomicBool,
    local_speech_active: AtomicBool,
    last_reason: Mutex<String>,
    reference_16k: Mutex<VecDeque<f32>>,
    capture: Mutex<Option<system_reference::SystemAudioReferenceCapture>>,
}

impl EchoGateShared {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            last_reason: Mutex::new("not_started".into()),
            ..Self::default()
        })
    }

    pub fn start_reference(self: &Arc<Self>) -> Result<(), String> {
        self.stop_reference();
        self.set_status(false, false, "starting");
        let capture = system_reference::SystemAudioReferenceCapture::start(Arc::clone(self))?;
        if let Ok(mut guard) = self.capture.lock() {
            *guard = Some(capture);
        }
        self.active.store(true, Ordering::SeqCst);
        self.reference_available.store(true, Ordering::SeqCst);
        self.set_reason("speaker_reference_ready");
        Ok(())
    }

    pub fn stop_reference(&self) {
        if let Ok(mut guard) = self.capture.lock() {
            *guard = None;
        }
        self.active.store(false, Ordering::SeqCst);
        self.reference_available.store(false, Ordering::SeqCst);
        self.local_speech_active.store(false, Ordering::SeqCst);
        if let Ok(mut samples) = self.reference_16k.lock() {
            samples.clear();
        }
        self.set_reason("stopped");
    }

    pub fn mark_reference_unavailable(&self, reason: impl Into<String>) {
        self.active.store(false, Ordering::SeqCst);
        self.reference_available.store(false, Ordering::SeqCst);
        self.local_speech_active.store(false, Ordering::SeqCst);
        self.set_reason(reason);
    }

    pub fn push_reference_samples_16k(&self, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }
        let Ok(mut buffer) = self.reference_16k.lock() else {
            return;
        };
        buffer.extend(samples.iter().map(|s| s.clamp(-1.0, 1.0)));
        while buffer.len() > REFERENCE_BUFFER_SAMPLES {
            buffer.pop_front();
        }
    }

    pub fn filter_mic_samples_16k(&self, mic_samples: &[f32]) -> EchoDecision {
        let decision = self.decide(mic_samples);
        self.local_speech_active.store(
            decision.allow && decision.mic_rms >= MIC_ACTIVE_RMS,
            Ordering::SeqCst,
        );
        self.set_reason(decision.reason);
        decision
    }

    pub fn local_speech_active(&self) -> bool {
        self.local_speech_active.load(Ordering::SeqCst)
    }

    pub fn is_filter_available(&self) -> bool {
        self.active.load(Ordering::SeqCst) && self.reference_available.load(Ordering::SeqCst)
    }

    pub fn status(&self) -> EchoGateStatus {
        EchoGateStatus {
            speaker_reference_available: self.reference_available.load(Ordering::SeqCst),
            echo_gate_active: self.active.load(Ordering::SeqCst),
            local_speech_active: self.local_speech_active.load(Ordering::SeqCst),
            last_gate_reason: self
                .last_reason
                .lock()
                .map(|s| s.clone())
                .unwrap_or_else(|_| "status_unavailable".into()),
        }
    }

    fn decide(&self, mic_samples: &[f32]) -> EchoDecision {
        let mic_rms = rms(mic_samples);
        if mic_rms < MIC_ACTIVE_RMS {
            return EchoDecision {
                allow: false,
                mic_rms,
                speaker_rms: 0.0,
                correlation: 0.0,
                reason: "mic_silent",
            };
        }

        if !self.is_filter_available() {
            return EchoDecision {
                allow: true,
                mic_rms,
                speaker_rms: 0.0,
                correlation: 0.0,
                reason: "reference_unavailable",
            };
        }

        let reference: Vec<f32> = self
            .reference_16k
            .lock()
            .map(|samples| samples.iter().copied().collect())
            .unwrap_or_default();
        if reference.len() < mic_samples.len().min(160) {
            return EchoDecision {
                allow: true,
                mic_rms,
                speaker_rms: 0.0,
                correlation: 0.0,
                reason: "reference_warming",
            };
        }

        let speaker_rms = rms(&reference[reference.len().saturating_sub(mic_samples.len())..]);
        if speaker_rms < SPEAKER_ACTIVE_RMS {
            return EchoDecision {
                allow: true,
                mic_rms,
                speaker_rms,
                correlation: 0.0,
                reason: "speaker_silent",
            };
        }

        let correlation = best_lag_correlation(mic_samples, &reference);
        if correlation >= ECHO_CORRELATION_THRESHOLD {
            EchoDecision {
                allow: false,
                mic_rms,
                speaker_rms,
                correlation,
                reason: "speaker_bleed",
            }
        } else {
            EchoDecision {
                allow: true,
                mic_rms,
                speaker_rms,
                correlation,
                reason: "local_speech",
            }
        }
    }

    fn set_status(&self, active: bool, available: bool, reason: impl Into<String>) {
        self.active.store(active, Ordering::SeqCst);
        self.reference_available.store(available, Ordering::SeqCst);
        self.set_reason(reason);
    }

    fn set_reason(&self, reason: impl Into<String>) {
        if let Ok(mut guard) = self.last_reason.lock() {
            *guard = reason.into();
        }
    }
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum = samples.iter().map(|s| s * s).sum::<f32>();
    (sum / samples.len() as f32).sqrt()
}

fn best_lag_correlation(mic_samples: &[f32], reference: &[f32]) -> f32 {
    if mic_samples.len() < 80 || reference.len() < mic_samples.len() {
        return 0.0;
    }

    let mut best = 0.0f32;
    let max_lag = MAX_ECHO_LAG_SAMPLES.min(reference.len().saturating_sub(mic_samples.len()));
    let mut lag = 0usize;
    while lag <= max_lag {
        let end = reference.len().saturating_sub(lag);
        let start = end.saturating_sub(mic_samples.len());
        if start < end {
            best = best.max(normalized_correlation(mic_samples, &reference[start..end]));
        }
        lag += LAG_STEP_SAMPLES;
    }
    best
}

fn normalized_correlation(a: &[f32], b: &[f32]) -> f32 {
    let len = a.len().min(b.len());
    if len == 0 {
        return 0.0;
    }
    let (mut dot, mut a_energy, mut b_energy) = (0.0f32, 0.0f32, 0.0f32);
    for i in 0..len {
        dot += a[i] * b[i];
        a_energy += a[i] * a[i];
        b_energy += b[i] * b[i];
    }
    if a_energy <= f32::EPSILON || b_energy <= f32::EPSILON {
        0.0
    } else {
        (dot / (a_energy.sqrt() * b_energy.sqrt())).abs()
    }
}

#[cfg(target_os = "macos")]
mod system_reference {
    use super::EchoGateShared;
    use std::sync::Arc;

    use screencapturekit::prelude::*;

    pub struct SystemAudioReferenceCapture {
        stream: SCStream,
    }

    impl SystemAudioReferenceCapture {
        pub fn start(gate: Arc<EchoGateShared>) -> Result<Self, String> {
            let content = SCShareableContent::get()
                .map_err(|e| format!("ScreenCaptureKit shareable content failed: {e}"))?;
            let display = content
                .displays()
                .into_iter()
                .next()
                .ok_or_else(|| "ScreenCaptureKit returned no display".to_string())?;
            let filter = SCContentFilter::create()
                .with_display(&display)
                .with_excluding_windows(&[])
                .build();
            let config = SCStreamConfiguration::new()
                .with_width(2)
                .with_height(2)
                .with_captures_audio(true)
                .with_sample_rate(16_000)
                .with_channel_count(1)
                .with_excludes_current_process_audio(true);

            let mut stream = SCStream::new(&filter, &config);
            let handler_gate = Arc::clone(&gate);
            let handler_id = stream.add_output_handler(
                move |sample: CMSampleBuffer, of_type: SCStreamOutputType| {
                    if of_type != SCStreamOutputType::Audio {
                        return;
                    }
                    if let Some(samples) = samples_from_buffer(&sample) {
                        handler_gate.push_reference_samples_16k(&samples);
                    }
                },
                SCStreamOutputType::Audio,
            );
            if handler_id.is_none() {
                return Err("ScreenCaptureKit failed to add audio output handler".into());
            }

            stream
                .start_capture()
                .map_err(|e| format!("ScreenCaptureKit audio capture failed: {e}"))?;
            Ok(Self { stream })
        }
    }

    impl Drop for SystemAudioReferenceCapture {
        fn drop(&mut self) {
            let _ = self.stream.stop_capture();
        }
    }

    fn samples_from_buffer(sample: &CMSampleBuffer) -> Option<Vec<f32>> {
        let _ = sample.make_data_ready();
        let audio = sample.audio_buffer_list()?;
        let mut out = Vec::new();
        for buffer in audio.iter() {
            out.extend(decode_pcm_f32(buffer.data()));
        }
        if out.is_empty() { None } else { Some(out) }
    }

    fn decode_pcm_f32(bytes: &[u8]) -> Vec<f32> {
        if bytes.len() % 4 == 0 {
            let f32_samples: Vec<f32> = bytes
                .chunks_exact(4)
                .map(|chunk| f32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                .filter(|s| s.is_finite())
                .map(|s| s.clamp(-1.0, 1.0))
                .collect();
            if !f32_samples.is_empty() {
                return f32_samples;
            }
        }
        bytes
            .chunks_exact(2)
            .map(|chunk| i16::from_ne_bytes([chunk[0], chunk[1]]) as f32 / 32_768.0)
            .collect()
    }
}

#[cfg(not(target_os = "macos"))]
mod system_reference {
    use super::EchoGateShared;
    use std::sync::Arc;

    #[derive(Default)]
    pub struct SystemAudioReferenceCapture;

    impl SystemAudioReferenceCapture {
        pub fn start(_gate: Arc<EchoGateShared>) -> Result<Self, String> {
            Err("system-output reference capture is macOS-only".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(len: usize, phase: f32) -> Vec<f32> {
        (0..len)
            .map(|i| (((i as f32 * 0.07) + phase).sin()) * 0.08)
            .collect()
    }

    #[test]
    fn echo_gate_drops_matching_speaker_reference() {
        let gate = EchoGateShared::new();
        gate.active.store(true, Ordering::SeqCst);
        gate.reference_available.store(true, Ordering::SeqCst);
        let signal = sine(1_600, 0.0);
        gate.push_reference_samples_16k(&signal);

        let decision = gate.filter_mic_samples_16k(&signal);
        assert!(!decision.allow, "{decision:?}");
        assert_eq!(decision.reason, "speaker_bleed");
    }

    #[test]
    fn echo_gate_forwards_local_speech_when_speaker_is_silent() {
        let gate = EchoGateShared::new();
        gate.active.store(true, Ordering::SeqCst);
        gate.reference_available.store(true, Ordering::SeqCst);
        gate.push_reference_samples_16k(&vec![0.0; 1_600]);

        let local = sine(1_600, 1.2);
        let decision = gate.filter_mic_samples_16k(&local);
        assert!(decision.allow, "{decision:?}");
        assert_eq!(decision.reason, "speaker_silent");
    }

    #[test]
    fn echo_gate_stays_conservative_on_overlap() {
        let gate = EchoGateShared::new();
        gate.active.store(true, Ordering::SeqCst);
        gate.reference_available.store(true, Ordering::SeqCst);
        let speaker = sine(1_600, 0.0);
        gate.push_reference_samples_16k(&speaker);
        let mic: Vec<f32> = speaker
            .iter()
            .enumerate()
            .map(|(i, s)| s + ((i as f32 * 0.11).cos() * 0.025))
            .collect();

        let decision = gate.filter_mic_samples_16k(&mic);
        assert!(!decision.allow, "{decision:?}");
    }
}
