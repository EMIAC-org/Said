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

#[cfg(any(target_os = "windows", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WindowsReferenceSampleFormat {
    F32,
    I16,
    I24,
    I32,
}

#[cfg(any(target_os = "windows", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WindowsReferenceMixFormat {
    channels: u16,
    sample_rate: u32,
    block_align: u16,
    bytes_per_sample: u16,
    sample_format: WindowsReferenceSampleFormat,
}

#[cfg(any(target_os = "windows", test))]
fn pcm_reference_format_for_bits(
    bits_per_sample: u16,
) -> Result<WindowsReferenceSampleFormat, String> {
    match bits_per_sample {
        16 => Ok(WindowsReferenceSampleFormat::I16),
        24 => Ok(WindowsReferenceSampleFormat::I24),
        32 => Ok(WindowsReferenceSampleFormat::I32),
        _ => Err(format!(
            "unsupported WASAPI speaker-reference PCM bits_per_sample={bits_per_sample}"
        )),
    }
}

#[cfg(any(target_os = "windows", test))]
fn min_bytes_per_windows_reference_sample(sample_format: WindowsReferenceSampleFormat) -> usize {
    match sample_format {
        WindowsReferenceSampleFormat::F32 | WindowsReferenceSampleFormat::I32 => 4,
        WindowsReferenceSampleFormat::I24 => 3,
        WindowsReferenceSampleFormat::I16 => 2,
    }
}

#[cfg(any(target_os = "windows", test))]
fn bytes_per_windows_reference_sample(
    channels: u16,
    block_align: u16,
    sample_format: WindowsReferenceSampleFormat,
) -> Option<usize> {
    let channels = channels.max(1) as usize;
    let block_align = block_align as usize;
    if block_align == 0 || block_align % channels != 0 {
        return None;
    }
    let bytes_per_sample = block_align / channels;
    if bytes_per_sample < min_bytes_per_windows_reference_sample(sample_format)
        || bytes_per_sample > 4
    {
        return None;
    }
    Some(bytes_per_sample)
}

#[cfg(any(target_os = "windows", test))]
fn decode_windows_reference_frames_to_mono(
    bytes: &[u8],
    frame_count: u32,
    channels: u16,
    block_align: u16,
    bytes_per_sample: u16,
    sample_format: WindowsReferenceSampleFormat,
) -> Vec<f32> {
    let channels = channels.max(1) as usize;
    let block_align = block_align as usize;
    let sample_bytes = bytes_per_sample as usize;
    if block_align == 0
        || sample_bytes == 0
        || sample_bytes < min_bytes_per_windows_reference_sample(sample_format)
        || sample_bytes > 4
    {
        return Vec::new();
    }
    let frames = (frame_count as usize).min(bytes.len() / block_align);
    let mut mono = Vec::with_capacity(frames);
    for frame_index in 0..frames {
        let frame_start = frame_index * block_align;
        let frame_end = frame_start + block_align;
        let mut sum = 0.0_f32;
        let mut seen = 0_usize;
        for channel_index in 0..channels {
            let offset = frame_start + channel_index * sample_bytes;
            if offset + sample_bytes > frame_end || offset + sample_bytes > bytes.len() {
                break;
            }
            sum += decode_windows_reference_sample(
                &bytes[offset..offset + sample_bytes],
                sample_format,
            );
            seen += 1;
        }
        if seen > 0 {
            mono.push(sum / seen as f32);
        }
    }
    mono
}

#[cfg(any(target_os = "windows", test))]
fn decode_windows_reference_sample(
    bytes: &[u8],
    sample_format: WindowsReferenceSampleFormat,
) -> f32 {
    match sample_format {
        WindowsReferenceSampleFormat::F32 => {
            if bytes.len() < 4 {
                return 0.0;
            }
            let sample = f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            if sample.is_finite() {
                sample.clamp(-1.0, 1.0)
            } else {
                0.0
            }
        }
        WindowsReferenceSampleFormat::I16
        | WindowsReferenceSampleFormat::I24
        | WindowsReferenceSampleFormat::I32 => decode_windows_reference_signed_pcm_container(bytes),
    }
}

#[cfg(any(target_os = "windows", test))]
fn decode_windows_reference_signed_pcm_container(bytes: &[u8]) -> f32 {
    match bytes.len() {
        2 => i16::from_le_bytes([bytes[0], bytes[1]]) as f32 / 32_768.0,
        3 => {
            let raw = (bytes[0] as i32) | ((bytes[1] as i32) << 8) | ((bytes[2] as i32) << 16);
            let signed = (raw << 8) >> 8;
            signed as f32 / 8_388_608.0
        }
        4 => i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f32 / 2_147_483_648.0,
        _ => 0.0,
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

#[cfg(target_os = "windows")]
mod system_reference {
    use super::{
        EchoGateShared, WindowsReferenceMixFormat, WindowsReferenceSampleFormat,
        bytes_per_windows_reference_sample, decode_windows_reference_frames_to_mono,
        pcm_reference_format_for_bits,
    };
    use std::sync::{Arc, mpsc};
    use std::thread;
    use std::time::Duration;

    pub struct SystemAudioReferenceCapture {
        stop_tx: Option<mpsc::Sender<()>>,
        join: Option<thread::JoinHandle<()>>,
    }

    impl SystemAudioReferenceCapture {
        pub fn start(gate: Arc<EchoGateShared>) -> Result<Self, String> {
            let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
            let (stop_tx, stop_rx) = mpsc::channel::<()>();
            let thread_gate = Arc::clone(&gate);
            let join = thread::Builder::new()
                .name("echo-system-reference-wasapi".to_string())
                .spawn(move || {
                    if let Err(err) = run_windows_reference_capture(thread_gate, stop_rx, ready_tx)
                    {
                        tracing::warn!("[echo_gate] WASAPI speaker reference stopped: {err}");
                    }
                })
                .map_err(|e| format!("failed to spawn WASAPI speaker-reference thread: {e}"))?;

            match ready_rx.recv_timeout(Duration::from_secs(3)) {
                Ok(Ok(())) => Ok(Self {
                    stop_tx: Some(stop_tx),
                    join: Some(join),
                }),
                Ok(Err(err)) => {
                    let _ = stop_tx.send(());
                    let _ = join.join();
                    Err(err)
                }
                Err(err) => {
                    let _ = stop_tx.send(());
                    let _ = join.join();
                    Err(format!(
                        "WASAPI speaker reference did not become ready: {err}"
                    ))
                }
            }
        }
    }

    impl Drop for SystemAudioReferenceCapture {
        fn drop(&mut self) {
            if let Some(tx) = self.stop_tx.take() {
                let _ = tx.send(());
            }
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
        }
    }

    fn run_windows_reference_capture(
        gate: Arc<EchoGateShared>,
        stop_rx: mpsc::Receiver<()>,
        ready_tx: mpsc::Sender<Result<(), String>>,
    ) -> Result<(), String> {
        let _com = initialize_wasapi_com().map_err(|e| {
            let message = format!("WASAPI COM init failed: {e}");
            let _ = ready_tx.send(Err(message.clone()));
            message
        })?;
        let (audio_client, capture_client, mix_format) =
            open_windows_reference_capture().map_err(|e| {
                let message = format!("WASAPI speaker reference open failed: {e}");
                let _ = ready_tx.send(Err(message.clone()));
                message
            })?;

        tracing::info!(
            native_rate = mix_format.sample_rate,
            native_channels = mix_format.channels,
            block_align = mix_format.block_align,
            bytes_per_sample = mix_format.bytes_per_sample,
            sample_format = ?mix_format.sample_format,
            "[echo_gate] opened WASAPI speaker reference"
        );

        if let Err(e) = unsafe { audio_client.Start() } {
            let message = format!("failed to start WASAPI speaker reference: {e}");
            let _ = ready_tx.send(Err(message.clone()));
            return Err(message);
        }

        let _ = ready_tx.send(Ok(()));
        let poll_interval = Duration::from_millis(10);
        loop {
            match stop_rx.try_recv() {
                Ok(()) | Err(mpsc::TryRecvError::Disconnected) => break,
                Err(mpsc::TryRecvError::Empty) => {}
            }

            if let Err(e) =
                unsafe { drain_windows_reference_packets(&capture_client, &mix_format, &gate) }
            {
                gate.mark_reference_unavailable(format!("WASAPI speaker reference failed: {e}"));
                let _ = unsafe { audio_client.Stop() };
                return Err(e);
            }
            thread::sleep(poll_interval);
        }

        let _ = unsafe { audio_client.Stop() };
        Ok(())
    }

    struct WasapiComGuard {
        uninitialize: bool,
    }

    impl Drop for WasapiComGuard {
        fn drop(&mut self) {
            if self.uninitialize {
                unsafe {
                    windows::Win32::System::Com::CoUninitialize();
                }
            }
        }
    }

    fn initialize_wasapi_com() -> Result<WasapiComGuard, String> {
        use windows::Win32::Foundation::{RPC_E_CHANGED_MODE, S_FALSE, S_OK};
        use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};

        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr.is_err() && hr != RPC_E_CHANGED_MODE {
            return Err(format!("{hr:?}"));
        }
        Ok(WasapiComGuard {
            uninitialize: hr == S_OK || hr == S_FALSE,
        })
    }

    fn open_windows_reference_capture() -> Result<
        (
            windows::Win32::Media::Audio::IAudioClient,
            windows::Win32::Media::Audio::IAudioCaptureClient,
            WindowsReferenceMixFormat,
        ),
        String,
    > {
        use windows::Win32::Media::Audio::{
            AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, IAudioCaptureClient,
            IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator, eConsole, eRender,
        };
        use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree};

        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                    .map_err(|e| format!("CoCreateInstance(MMDeviceEnumerator) failed: {e}"))?;
            let device = enumerator
                .GetDefaultAudioEndpoint(eRender, eConsole)
                .map_err(|e| format!("GetDefaultAudioEndpoint(render/console) failed: {e}"))?;
            let audio_client: IAudioClient = device
                .Activate(CLSCTX_ALL, None)
                .map_err(|e| format!("Activate(IAudioClient) failed: {e}"))?;
            let mix_format_ptr = audio_client
                .GetMixFormat()
                .map_err(|e| format!("GetMixFormat failed: {e}"))?;
            if mix_format_ptr.is_null() {
                return Err("GetMixFormat returned null".to_string());
            }

            let mix_format = match wasapi_reference_mix_format_from_ptr(mix_format_ptr) {
                Ok(format) => format,
                Err(e) => {
                    CoTaskMemFree(Some(mix_format_ptr as *const std::ffi::c_void));
                    return Err(e);
                }
            };

            let init_result = audio_client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK,
                10_000_000,
                0,
                mix_format_ptr,
                None,
            );
            CoTaskMemFree(Some(mix_format_ptr as *const std::ffi::c_void));
            init_result.map_err(|e| format!("IAudioClient::Initialize(loopback) failed: {e}"))?;

            let capture_client: IAudioCaptureClient = audio_client
                .GetService()
                .map_err(|e| format!("GetService(IAudioCaptureClient) failed: {e}"))?;
            Ok((audio_client, capture_client, mix_format))
        }
    }

    unsafe fn wasapi_reference_mix_format_from_ptr(
        format_ptr: *const windows::Win32::Media::Audio::WAVEFORMATEX,
    ) -> Result<WindowsReferenceMixFormat, String> {
        use windows::Win32::Media::Audio::{WAVE_FORMAT_PCM, WAVEFORMATEXTENSIBLE};
        use windows::Win32::Media::KernelStreaming::{
            KSDATAFORMAT_SUBTYPE_PCM, WAVE_FORMAT_EXTENSIBLE,
        };
        use windows::Win32::Media::Multimedia::{
            KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, WAVE_FORMAT_IEEE_FLOAT,
        };

        let format = unsafe { *format_ptr };
        let channels = format.nChannels;
        let sample_rate = format.nSamplesPerSec;
        let block_align = format.nBlockAlign;
        let bits_per_sample = format.wBitsPerSample;
        let format_tag = format.wFormatTag;
        if channels == 0 || sample_rate == 0 || block_align == 0 {
            return Err(format!(
                "invalid WASAPI speaker-reference mix format: channels={channels}, sample_rate={sample_rate}, block_align={block_align}"
            ));
        }

        let sample_format = if format_tag == WAVE_FORMAT_IEEE_FLOAT as u16 {
            WindowsReferenceSampleFormat::F32
        } else if format_tag == WAVE_FORMAT_PCM as u16 {
            pcm_reference_format_for_bits(bits_per_sample)?
        } else if format_tag == WAVE_FORMAT_EXTENSIBLE as u16
            && format.cbSize as usize
                >= std::mem::size_of::<WAVEFORMATEXTENSIBLE>().saturating_sub(std::mem::size_of::<
                    windows::Win32::Media::Audio::WAVEFORMATEX,
                >())
        {
            let extensible = unsafe { *(format_ptr as *const WAVEFORMATEXTENSIBLE) };
            if extensible.SubFormat == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT {
                WindowsReferenceSampleFormat::F32
            } else if extensible.SubFormat == KSDATAFORMAT_SUBTYPE_PCM {
                let valid_bits = unsafe { extensible.Samples.wValidBitsPerSample };
                pcm_reference_format_for_bits(if valid_bits == 0 {
                    bits_per_sample
                } else {
                    valid_bits
                })?
            } else {
                return Err(format!(
                    "unsupported WASAPI speaker-reference extensible subformat: {:?}",
                    extensible.SubFormat
                ));
            }
        } else {
            return Err(format!(
                "unsupported WASAPI speaker-reference mix format tag={format_tag}, bits_per_sample={bits_per_sample}"
            ));
        };

        let bytes_per_sample =
            bytes_per_windows_reference_sample(channels, block_align, sample_format).ok_or_else(
                || {
                    format!(
                        "invalid WASAPI speaker-reference sample container: channels={channels}, block_align={block_align}, sample_format={sample_format:?}"
                    )
                },
            )? as u16;

        Ok(WindowsReferenceMixFormat {
            channels,
            sample_rate,
            block_align,
            bytes_per_sample,
            sample_format,
        })
    }

    unsafe fn drain_windows_reference_packets(
        capture_client: &windows::Win32::Media::Audio::IAudioCaptureClient,
        mix_format: &WindowsReferenceMixFormat,
        gate: &EchoGateShared,
    ) -> Result<(), String> {
        use windows::Win32::Media::Audio::AUDCLNT_BUFFERFLAGS_SILENT;

        let mut packet_frames = unsafe { capture_client.GetNextPacketSize() }
            .map_err(|e| format!("WASAPI GetNextPacketSize failed: {e}"))?;
        while packet_frames > 0 {
            let mut data: *mut u8 = std::ptr::null_mut();
            let mut frames: u32 = 0;
            let mut flags: u32 = 0;
            unsafe { capture_client.GetBuffer(&mut data, &mut frames, &mut flags, None, None) }
                .map_err(|e| format!("WASAPI GetBuffer failed: {e}"))?;

            if frames > 0 {
                let mono = if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                    vec![0.0; frames as usize]
                } else {
                    let byte_len = frames as usize * mix_format.block_align as usize;
                    let bytes = unsafe { std::slice::from_raw_parts(data.cast_const(), byte_len) };
                    decode_windows_reference_frames_to_mono(
                        bytes,
                        frames,
                        mix_format.channels,
                        mix_format.block_align,
                        mix_format.bytes_per_sample,
                        mix_format.sample_format,
                    )
                };
                let resampled = said_recorder::resample_to_16k(&mono, mix_format.sample_rate);
                gate.push_reference_samples_16k(&resampled);
            }

            unsafe { capture_client.ReleaseBuffer(frames) }
                .map_err(|e| format!("WASAPI ReleaseBuffer failed: {e}"))?;
            packet_frames = unsafe { capture_client.GetNextPacketSize() }
                .map_err(|e| format!("WASAPI GetNextPacketSize failed: {e}"))?;
        }
        Ok(())
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod system_reference {
    use super::EchoGateShared;
    use std::sync::Arc;

    #[derive(Default)]
    pub struct SystemAudioReferenceCapture;

    impl SystemAudioReferenceCapture {
        pub fn start(_gate: Arc<EchoGateShared>) -> Result<Self, String> {
            Err("system-output reference capture is only supported on macOS and Windows".into())
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

    #[test]
    fn windows_reference_decoder_downmixes_f32_stereo() {
        let mut bytes = Vec::new();
        for sample in [0.25_f32, 0.75, -0.5, 0.0] {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }

        let mono = decode_windows_reference_frames_to_mono(
            &bytes,
            2,
            2,
            8,
            4,
            WindowsReferenceSampleFormat::F32,
        );
        assert_eq!(mono.len(), 2);
        assert!((mono[0] - 0.5).abs() < 0.0001);
        assert!((mono[1] - -0.25).abs() < 0.0001);
    }

    #[test]
    fn windows_reference_decoder_handles_packed_i24() {
        let bytes = [0x00, 0x00, 0x40, 0x00, 0x00, 0xC0];
        let mono = decode_windows_reference_frames_to_mono(
            &bytes,
            2,
            1,
            3,
            3,
            WindowsReferenceSampleFormat::I24,
        );
        assert_eq!(mono.len(), 2);
        assert!((mono[0] - 0.5).abs() < 0.0001);
        assert!((mono[1] - -0.5).abs() < 0.0001);
    }

    #[test]
    fn windows_reference_decoder_handles_left_aligned_i24_in_i32_container() {
        let bytes = [
            0x00, 0x00, 0x00, 0x40, // +0.5 as 24 valid bits left-aligned in i32
            0x00, 0x00, 0x00, 0xC0, // -0.5 as 24 valid bits left-aligned in i32
        ];
        let mono = decode_windows_reference_frames_to_mono(
            &bytes,
            2,
            1,
            4,
            4,
            WindowsReferenceSampleFormat::I24,
        );
        assert_eq!(mono.len(), 2);
        assert!((mono[0] - 0.5).abs() < 0.0001);
        assert!((mono[1] - -0.5).abs() < 0.0001);
    }

    #[test]
    fn windows_reference_container_width_comes_from_block_align() {
        assert_eq!(
            bytes_per_windows_reference_sample(2, 8, WindowsReferenceSampleFormat::I24),
            Some(4)
        );
        assert_eq!(
            bytes_per_windows_reference_sample(2, 6, WindowsReferenceSampleFormat::I24),
            Some(3)
        );
        assert_eq!(
            bytes_per_windows_reference_sample(2, 4, WindowsReferenceSampleFormat::I24),
            None
        );
    }
}
