use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::io::Cursor;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cfg(target_os = "macos")]
mod macos_voice_processing;

// ── Recording constants ───────────────────────────────────────────────────────

pub const SAMPLE_RATE: u32 = 16_000;
pub const CHANNELS: u16 = 1;
pub const MIN_DURATION_S: f32 = 0.5;
const STOP_REPLY_TIMEOUT: Duration = Duration::from_secs(3);
const ALLOW_BLUETOOTH_MIC_ENV: &str = "AIRNOTE_ALLOW_BLUETOOTH_MIC";
#[cfg(target_os = "macos")]
const MACOS_VOICE_PROCESSING_ENV: &str = "AIRNOTE_MACOS_VOICE_PROCESSING";

// ── Internal command ──────────────────────────────────────────────────────────

enum RecCmd {
    Stop(mpsc::Sender<(Vec<f32>, u32)>),
}

// ── Resample helper ───────────────────────────────────────────────────────────

/// Downsample/upsample `samples` from `src_rate` to `SAMPLE_RATE` (16 kHz)
/// using linear interpolation.  Pure-Rust, no external crate needed.
pub fn resample_to_16k(samples: &[f32], src_rate: u32) -> Vec<f32> {
    if src_rate == SAMPLE_RATE {
        return samples.to_vec();
    }
    let ratio = src_rate as f64 / SAMPLE_RATE as f64;
    let out_len = (samples.len() as f64 / ratio).ceil() as usize;
    (0..out_len)
        .map(|i| {
            let pos = i as f64 * ratio;
            let idx = pos as usize;
            let frac = (pos - idx as f64) as f32;
            let a = samples.get(idx).copied().unwrap_or(0.0);
            let b = samples.get(idx + 1).copied().unwrap_or(a);
            a + (b - a) * frac
        })
        .collect()
}

fn fan_out_mono(
    mono: Vec<f32>,
    frames_cb: &Arc<Mutex<Vec<f32>>>,
    chunk_tx_cb: &mpsc::SyncSender<Vec<f32>>,
    level_tx_cb: &mpsc::SyncSender<f32>,
) {
    if mono.is_empty() {
        return;
    }
    match frames_cb.lock() {
        Ok(mut frames) => frames.extend_from_slice(&mono),
        Err(poison) => {
            eprintln!("[rec] recovered poisoned audio buffer lock in callback");
            poison.into_inner().extend_from_slice(&mono);
        }
    }
    let _ = chunk_tx_cb.try_send(mono.clone());
    let sum_sq = mono.iter().map(|s| s * s).sum::<f32>();
    let rms = (sum_sq / mono.len() as f32).sqrt();
    let boosted = (rms * 9.0).clamp(0.0, 1.0);
    let _ = level_tx_cb.try_send(boosted);
}

// ── Chunk receiver ────────────────────────────────────────────────────────────

/// A live handle to raw audio chunks as they arrive from the microphone.
/// Used by local speech transcription and recovery diagnostics.
pub struct ChunkReceiver {
    pub rx: mpsc::Receiver<Vec<f32>>,
    pub native_rate: u32,
}

/// Live microphone amplitude for UI visualizers.
/// Values are normalized to roughly 0.0–1.0 and are intentionally lossy.
pub struct LevelReceiver {
    pub rx: mpsc::Receiver<f32>,
}

pub type StopReceiver = mpsc::Receiver<(Vec<f32>, u32)>;

// ── Public recorder ───────────────────────────────────────────────────────────

pub struct AudioRecorder {
    cmd_tx: Option<mpsc::Sender<RecCmd>>,
    /// Held until `take_chunk_receiver()` is called — then moved to the WS task.
    chunk_rx: Option<mpsc::Receiver<Vec<f32>>>,
    /// Recorder's own copy of the chunk sender; dropped explicitly in `stop()`
    /// so the WS task sees the channel close when the cpal stream also ends.
    chunk_tx: Option<mpsc::SyncSender<Vec<f32>>>,
    level_rx: Option<mpsc::Receiver<f32>>,
    level_tx: Option<mpsc::SyncSender<f32>>,
    native_rate: Option<u32>,
}

impl AudioRecorder {
    pub fn new() -> Self {
        #[cfg(target_os = "macos")]
        if macos_voice_processing_capture_enabled() {
            macos_voice_processing::prewarm();
        }

        Self {
            cmd_tx: None,
            chunk_rx: None,
            chunk_tx: None,
            level_rx: None,
            level_tx: None,
            native_rate: None,
        }
    }

    pub fn start(&mut self) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        if macos_voice_processing_capture_enabled() {
            match self.start_macos_voice_processing() {
                Ok(()) => return Ok(()),
                Err(e) => {
                    eprintln!(
                        "[rec] Apple voice-processing capture unavailable ({e}); falling back to raw CPAL capture"
                    );
                }
            }
        }

        self.start_cpal()
    }

    fn start_cpal(&mut self) -> Result<(), String> {
        let host = cpal::default_host();
        let device = select_input_device(&host)?;

        let frames: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
        let frames_for_reply = Arc::clone(&frames);

        let (cmd_tx, cmd_rx) = mpsc::channel::<RecCmd>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<u32, String>>();

        // Chunk channel for live local diagnostics: buffer 256 cpal frames.
        let (chunk_tx, chunk_rx) = mpsc::sync_channel::<Vec<f32>>(256);
        let chunk_tx_cb = chunk_tx.clone(); // moved into cpal callback

        let (level_tx, level_rx) = mpsc::sync_channel::<f32>(64);
        let level_tx_cb = level_tx.clone();

        std::thread::spawn(move || {
            let device_name = device.name().unwrap_or_else(|_| "unknown".into());

            let default_config = match device.default_input_config() {
                Ok(c) => c,
                Err(e) => {
                    let _ = ready_tx.send(Err(format!("no default input config: {e}")));
                    return;
                }
            };

            // Use the device's actual config exactly — Windows WASAPI in shared
            // mode rejects any mismatch (sample format, channel count, rate).
            // We downmix multi-channel input to mono and convert non-F32 sample
            // formats to F32 inside the callback so the rest of the pipeline
            // keeps operating on mono F32.
            let native_rate = default_config.sample_rate().0;
            let native_channels = default_config.channels();
            let sample_format = default_config.sample_format();
            let config = default_config.config();

            // Closure: take raw interleaved samples in their native format and
            // produce a Vec<f32> of mono F32 samples for the rest of the pipeline.
            let to_mono_f32_from_f32 = move |data: &[f32], channels: u16| -> Vec<f32> {
                if channels <= 1 {
                    return data.to_vec();
                }
                let ch = channels as usize;
                let mut out = Vec::with_capacity(data.len() / ch);
                for frame in data.chunks_exact(ch) {
                    let sum: f32 = frame.iter().sum();
                    out.push(sum / ch as f32);
                }
                out
            };
            let to_mono_f32_from_i16 = move |data: &[i16], channels: u16| -> Vec<f32> {
                let ch = channels.max(1) as usize;
                if ch == 1 {
                    return data.iter().map(|&s| s as f32 / 32768.0).collect();
                }
                let mut out = Vec::with_capacity(data.len() / ch);
                for frame in data.chunks_exact(ch) {
                    let sum: i32 = frame.iter().map(|&s| s as i32).sum();
                    let avg = (sum as f32) / (ch as f32);
                    out.push(avg / 32768.0);
                }
                out
            };
            let to_mono_f32_from_u16 = move |data: &[u16], channels: u16| -> Vec<f32> {
                let ch = channels.max(1) as usize;
                let normalize = |s: u16| -> f32 { (s as f32 - 32768.0) / 32768.0 };
                if ch == 1 {
                    return data.iter().map(|&s| normalize(s)).collect();
                }
                let mut out = Vec::with_capacity(data.len() / ch);
                for frame in data.chunks_exact(ch) {
                    let avg = frame.iter().map(|&s| normalize(s)).sum::<f32>() / ch as f32;
                    out.push(avg);
                }
                out
            };

            // Macro-free dispatch: one match-arm per sample format builds a
            // typed `build_input_stream` call. Each closure normalises into the
            // mono-F32 form expected downstream.
            let frames_cb = Arc::clone(&frames_for_reply);
            let chunk_tx_cb_arc = std::sync::Arc::new(chunk_tx_cb);
            let level_tx_cb_arc = std::sync::Arc::new(level_tx_cb);

            let err_cb = |err: cpal::StreamError| eprintln!("[rec] stream error: {err}");
            let build_result = match sample_format {
                cpal::SampleFormat::F32 => {
                    let frames_cb = Arc::clone(&frames_cb);
                    let chunk_tx_cb = Arc::clone(&chunk_tx_cb_arc);
                    let level_tx_cb = Arc::clone(&level_tx_cb_arc);
                    device.build_input_stream(
                        &config,
                        move |data: &[f32], _: &cpal::InputCallbackInfo| {
                            let mono = to_mono_f32_from_f32(data, native_channels);
                            fan_out_mono(mono, &frames_cb, &chunk_tx_cb, &level_tx_cb);
                        },
                        err_cb,
                        None,
                    )
                }
                cpal::SampleFormat::I16 => {
                    let frames_cb = Arc::clone(&frames_cb);
                    let chunk_tx_cb = Arc::clone(&chunk_tx_cb_arc);
                    let level_tx_cb = Arc::clone(&level_tx_cb_arc);
                    device.build_input_stream(
                        &config,
                        move |data: &[i16], _: &cpal::InputCallbackInfo| {
                            let mono = to_mono_f32_from_i16(data, native_channels);
                            fan_out_mono(mono, &frames_cb, &chunk_tx_cb, &level_tx_cb);
                        },
                        err_cb,
                        None,
                    )
                }
                cpal::SampleFormat::U16 => {
                    let frames_cb = Arc::clone(&frames_cb);
                    let chunk_tx_cb = Arc::clone(&chunk_tx_cb_arc);
                    let level_tx_cb = Arc::clone(&level_tx_cb_arc);
                    device.build_input_stream(
                        &config,
                        move |data: &[u16], _: &cpal::InputCallbackInfo| {
                            let mono = to_mono_f32_from_u16(data, native_channels);
                            fan_out_mono(mono, &frames_cb, &chunk_tx_cb, &level_tx_cb);
                        },
                        err_cb,
                        None,
                    )
                }
                other => {
                    let _ = ready_tx.send(Err(format!(
                        "unsupported sample format {other:?} — expected f32/i16/u16"
                    )));
                    return;
                }
            };

            let stream = match build_result {
                Ok(s) => s,
                Err(e) => {
                    let _ = ready_tx.send(Err(format!(
                        "failed to open audio stream: {e} (config: {native_channels}ch @ {native_rate} Hz, {sample_format:?})"
                    )));
                    return;
                }
            };

            if let Err(e) = stream.play() {
                let _ = ready_tx.send(Err(format!("failed to start stream: {e}")));
                return;
            }

            let _ = ready_tx.send(Ok(native_rate));
            println!("[rec] opened input '{device_name}' at {native_rate}Hz {sample_format:?}");

            if let Ok(RecCmd::Stop(reply)) = cmd_rx.recv() {
                let teardown_started = std::time::Instant::now();
                // Pause before drop so CoreAudio is asked to stop IO explicitly.
                // On macOS this is more reliable than relying on Drop alone,
                // especially when Bluetooth devices are connected and CoreAudio
                // device routing is being reconfigured.
                if let Err(e) = stream.pause() {
                    eprintln!("[rec] failed to pause input stream before drop: {e}");
                }
                // `stream` drops here → chunk_tx_cb drops → all senders gone → chunk_rx sees close
                drop(stream);
                let teardown_ms = teardown_started.elapsed().as_millis();
                if teardown_ms >= 100 {
                    eprintln!("[rec] input stream teardown took {teardown_ms}ms");
                }
                let data = match frames_for_reply.lock() {
                    Ok(frames) => frames.clone(),
                    Err(poison) => {
                        eprintln!("[rec] recovered poisoned audio buffer lock while stopping");
                        poison.into_inner().clone()
                    }
                };
                let _ = reply.send((data, native_rate));
            }
        });

        match ready_rx.recv() {
            Ok(Ok(rate)) => {
                self.native_rate = Some(rate);
                self.chunk_tx = Some(chunk_tx);
                self.chunk_rx = Some(chunk_rx);
                self.level_tx = Some(level_tx);
                self.level_rx = Some(level_rx);
                println!("[rec] opened CPAL capture at {rate}Hz");
            }
            Ok(Err(e)) => return Err(e),
            Err(_) => return Err("recording thread died".into()),
        }

        self.cmd_tx = Some(cmd_tx);
        println!("[rec] recording … press hotkey again to stop");
        Ok(())
    }

    #[cfg(target_os = "macos")]
    fn start_macos_voice_processing(&mut self) -> Result<(), String> {
        if !macos_default_input_allows_voice_processing()? {
            return Err("default input is not a safe VoiceProcessingIO target".into());
        }

        let frames: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
        let (cmd_tx, cmd_rx) = mpsc::channel::<RecCmd>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<u32, String>>();

        let (chunk_tx, chunk_rx) = mpsc::sync_channel::<Vec<f32>>(256);
        let (level_tx, level_rx) = mpsc::sync_channel::<f32>(64);

        macos_voice_processing::spawn_capture_thread(
            Arc::clone(&frames),
            cmd_rx,
            ready_tx,
            chunk_tx.clone(),
            level_tx.clone(),
        );

        match ready_rx.recv() {
            Ok(Ok(rate)) => {
                self.native_rate = Some(rate);
                self.chunk_tx = Some(chunk_tx);
                self.chunk_rx = Some(chunk_rx);
                self.level_tx = Some(level_tx);
                self.level_rx = Some(level_rx);
                self.cmd_tx = Some(cmd_tx);
                println!("[rec] recording with Apple voice processing");
                Ok(())
            }
            Ok(Err(e)) => Err(e),
            Err(_) => Err("voice-processing recording thread died".into()),
        }
    }

    /// Take the chunk receiver for local speech diagnostics.
    /// Can only be called once per recording session (after `start()`).
    pub fn take_chunk_receiver(&mut self) -> Option<ChunkReceiver> {
        let rx = self.chunk_rx.take()?;
        let native_rate = self.native_rate?;
        Some(ChunkReceiver { rx, native_rate })
    }

    pub fn take_level_receiver(&mut self) -> Option<LevelReceiver> {
        let rx = self.level_rx.take()?;
        Some(LevelReceiver { rx })
    }

    pub fn initiate_stop(&mut self) -> Option<StopReceiver> {
        let cmd_tx = self.cmd_tx.take()?;

        // Drop our copy of the chunk sender BEFORE the recording thread exits.
        // The cpal-callback copy will drop when the stream drops inside the thread.
        // Once both senders are gone the chunk_rx (held by the WS task) sees EOF.
        drop(self.chunk_tx.take());
        drop(self.level_tx.take());

        let (reply_tx, reply_rx) = mpsc::channel();
        let _ = cmd_tx.send(RecCmd::Stop(reply_tx));
        Some(reply_rx)
    }

    /// True while this recorder still owns handles that can keep the platform
    /// input stream alive. Used by desktop-side release cleanup to catch missed
    /// hotkey-release transitions without touching CoreAudio from the main thread.
    pub fn mic_stream_held(&self) -> bool {
        self.cmd_tx.is_some() || self.chunk_tx.is_some() || self.level_tx.is_some()
    }

    /// Best-effort emergency release for a recorder that still owns its mic
    /// stream after the app already moved past the normal stop point. The caller
    /// should drain the returned receiver on a background thread and discard it.
    pub fn release_mic_stream(&mut self) -> Option<StopReceiver> {
        self.initiate_stop()
    }

    pub fn collect_wav_result(reply_rx: StopReceiver) -> Result<Vec<u8>, String> {
        let (samples_f32, native_rate) = match reply_rx.recv_timeout(STOP_REPLY_TIMEOUT) {
            Ok(reply) => reply,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let msg = format!(
                    "recorder stop timed out after {}ms",
                    STOP_REPLY_TIMEOUT.as_millis()
                );
                eprintln!("[rec] {msg}");
                return Err(msg);
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("recording thread disconnected while stopping".to_string());
            }
        };

        if samples_f32.is_empty() {
            println!("[rec] no audio captured");
            return Err("no audio captured".to_string());
        }

        let duration = samples_f32.len() as f32 / native_rate as f32;
        let max_amp = samples_f32.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        println!("[rec] {duration:.1}s recorded ({native_rate}Hz → 16kHz, peak={max_amp:.4})");

        if max_amp < 0.0001 {
            eprintln!("[rec] audio is silence — microphone permission not granted?");
            #[cfg(target_os = "macos")]
            eprintln!("[rec]   System Settings → Privacy & Security → Microphone");
            #[cfg(target_os = "windows")]
            eprintln!("[rec]   Settings → Privacy & security → Microphone (allow desktop apps)");
            return Err("audio is silence".to_string());
        }

        if duration < MIN_DURATION_S {
            println!("[rec] too short — ignored");
            return Err("recording too short".to_string());
        }

        // ── P1: Resample to 16 kHz (smaller WAV → faster local transcription) ──
        let resampled = resample_to_16k(&samples_f32, native_rate);

        // Convert F32 → I16 WAV at 16 kHz
        let mut buf = Cursor::new(Vec::new());
        let spec = hound::WavSpec {
            channels: CHANNELS,
            sample_rate: SAMPLE_RATE, // 16_000 Hz
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer =
            hound::WavWriter::new(&mut buf, spec).map_err(|e| format!("wav writer: {e}"))?;
        for &sample in &resampled {
            let clamped = sample.clamp(-1.0, 1.0);
            writer
                .write_sample((clamped * 32767.0) as i16)
                .map_err(|e| format!("wav sample: {e}"))?;
        }
        writer
            .finalize()
            .map_err(|e| format!("wav finalize: {e}"))?;

        Ok(buf.into_inner())
    }

    pub fn collect_wav(reply_rx: StopReceiver) -> Option<Vec<u8>> {
        Self::collect_wav_result(reply_rx).ok()
    }

    pub fn stop(&mut self) -> Option<Vec<u8>> {
        let reply_rx = self.initiate_stop()?;
        Self::collect_wav(reply_rx)
    }

    pub fn preflight() -> Result<String, String> {
        let host = cpal::default_host();
        let device = select_input_device(&host)
            .map_err(|_| "no input device found — check microphone connection".to_string())?;
        let name = device.name().unwrap_or_else(|_| "unknown".into());
        Ok(name)
    }
}

impl Default for AudioRecorder {
    fn default() -> Self {
        Self::new()
    }
}

/// Pick a microphone without forcing Bluetooth headsets into low-quality call
/// mode when a local/USB microphone is available.
pub fn select_input_device(host: &cpal::Host) -> Result<cpal::Device, String> {
    let default = host.default_input_device().ok_or("no input device found")?;
    let default_name = device_name(&default);

    if bluetooth_mic_allowed() || !should_avoid_input(&default_name) {
        return Ok(default);
    }

    match fallback_input_device(host, &default_name) {
        Some(device) => {
            let fallback_name = device_name(&device);
            eprintln!(
                "[rec] default input '{default_name}' looks like a Bluetooth headset; using '{fallback_name}' to avoid headset audio mode"
            );
            Ok(device)
        }
        None => {
            eprintln!(
                "[rec] default input '{default_name}' looks like a Bluetooth headset, but no alternate microphone is available"
            );
            Ok(default)
        }
    }
}

fn device_name(device: &cpal::Device) -> String {
    device.name().unwrap_or_else(|_| "unknown".into())
}

fn fallback_input_device(host: &cpal::Host, default_name: &str) -> Option<cpal::Device> {
    let default_key = comparable_device_name(default_name);
    let mut first_usable = None;

    let devices = match host.input_devices() {
        Ok(devices) => devices,
        Err(e) => {
            eprintln!("[rec] failed to enumerate input devices: {e}");
            return None;
        }
    };

    for device in devices {
        let name = device_name(&device);
        if comparable_device_name(&name) == default_key
            || input_name_looks_bluetooth(&name)
            || input_name_looks_virtual(&name)
            || device.default_input_config().is_err()
        {
            continue;
        }

        if input_name_looks_local_mic(&name) {
            return Some(device);
        }

        if first_usable.is_none() {
            first_usable = Some(device);
        }
    }

    first_usable
}

fn should_avoid_input(default_name: &str) -> bool {
    #[cfg(target_os = "macos")]
    if let Ok(true) = macos_audio::default_input_is_bluetooth() {
        return true;
    }

    input_name_looks_bluetooth(default_name)
}

fn bluetooth_mic_allowed() -> bool {
    std::env::var(ALLOW_BLUETOOTH_MIC_ENV)
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn macos_voice_processing_capture_enabled() -> bool {
    std::env::var(MACOS_VOICE_PROCESSING_ENV)
        .map(|value| {
            !matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off"
            )
        })
        .unwrap_or(true)
}

#[cfg(target_os = "macos")]
fn macos_default_input_allows_voice_processing() -> Result<bool, String> {
    let host = cpal::default_host();
    let default = host.default_input_device().ok_or("no input device found")?;
    let default_name = device_name(&default);

    if input_name_looks_virtual(&default_name) {
        return Ok(false);
    }

    if !bluetooth_mic_allowed() && should_avoid_input(&default_name) {
        return Ok(false);
    }

    Ok(default.default_input_config().is_ok())
}

fn input_name_looks_bluetooth(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let compact = compact_device_name(&lower);
    const NEEDLES: &[&str] = &[
        "airpods",
        "airpod",
        "beats",
        "bluetooth",
        "hands-free",
        "hands free",
        "handsfree",
        "quietcomfort",
        "wh-1000",
        "wf-1000",
        "linkbuds",
        "galaxy buds",
        "pixel buds",
        "oneplus buds",
        "nothing ear",
        "freebuds",
    ];
    const COMPACT_NEEDLES: &[&str] = &[
        "wh1000",
        "wf1000",
        "galaxybuds",
        "pixelbuds",
        "oneplusbuds",
        "nothingear",
        "freebuds",
        "quietcomfort",
    ];

    NEEDLES.iter().any(|needle| lower.contains(needle))
        || COMPACT_NEEDLES
            .iter()
            .any(|needle| compact.contains(needle))
}

fn input_name_looks_local_mic(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("macbook")
        || lower.contains("built-in")
        || lower.contains("internal microphone")
        || lower.contains("studio display")
        || lower.contains("display microphone")
}

fn input_name_looks_virtual(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [
        "aggregate device",
        "background music",
        "blackhole",
        "loopback",
        "multi-output",
        "soundflower",
        "virtual",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn comparable_device_name(name: &str) -> String {
    name.trim().to_ascii_lowercase()
}

fn compact_device_name(name: &str) -> String {
    name.chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect()
}

#[cfg(target_os = "macos")]
mod macos_audio {
    use std::ffi::c_void;

    type AudioObjectID = u32;
    type AudioDeviceID = u32;
    type OSStatus = i32;
    type Boolean = u8;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct AudioObjectPropertyAddress {
        m_selector: u32,
        m_scope: u32,
        m_element: u32,
    }

    const K_AUDIO_OBJECT_SYSTEM_OBJECT: AudioObjectID = 1;
    const K_AUDIO_HARDWARE_PROPERTY_DEFAULT_INPUT_DEVICE: u32 = fourcc(*b"dIn ");
    const K_AUDIO_DEVICE_PROPERTY_TRANSPORT_TYPE: u32 = fourcc(*b"tran");
    const K_AUDIO_DEVICE_TRANSPORT_TYPE_BLUETOOTH: u32 = fourcc(*b"blue");
    const K_AUDIO_DEVICE_TRANSPORT_TYPE_BLUETOOTH_LE: u32 = fourcc(*b"blea");
    const K_AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL: u32 = fourcc(*b"glob");
    const K_AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN: u32 = 0;

    const fn fourcc(bytes: [u8; 4]) -> u32 {
        ((bytes[0] as u32) << 24)
            | ((bytes[1] as u32) << 16)
            | ((bytes[2] as u32) << 8)
            | bytes[3] as u32
    }

    #[link(name = "CoreAudio", kind = "framework")]
    unsafe extern "C" {
        fn AudioObjectGetPropertyData(
            in_object_id: AudioObjectID,
            in_address: *const AudioObjectPropertyAddress,
            in_qualifier_data_size: u32,
            in_qualifier_data: *const c_void,
            io_data_size: *mut u32,
            out_data: *mut c_void,
        ) -> OSStatus;

        fn AudioObjectHasProperty(
            in_object_id: AudioObjectID,
            in_address: *const AudioObjectPropertyAddress,
        ) -> Boolean;
    }

    pub fn default_input_is_bluetooth() -> Result<bool, String> {
        let transport = default_input_transport_type()?;
        Ok(is_bluetooth_transport(transport))
    }

    fn is_bluetooth_transport(transport: u32) -> bool {
        matches!(
            transport,
            K_AUDIO_DEVICE_TRANSPORT_TYPE_BLUETOOTH | K_AUDIO_DEVICE_TRANSPORT_TYPE_BLUETOOTH_LE
        )
    }

    fn default_input_device() -> Result<AudioDeviceID, String> {
        let address = AudioObjectPropertyAddress {
            m_selector: K_AUDIO_HARDWARE_PROPERTY_DEFAULT_INPUT_DEVICE,
            m_scope: K_AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL,
            m_element: K_AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN,
        };
        let mut device: AudioDeviceID = 0;
        let mut size = std::mem::size_of::<AudioDeviceID>() as u32;
        let status = unsafe {
            AudioObjectGetPropertyData(
                K_AUDIO_OBJECT_SYSTEM_OBJECT,
                &address,
                0,
                std::ptr::null(),
                &mut size,
                (&mut device as *mut AudioDeviceID).cast::<c_void>(),
            )
        };
        if status == 0 && device != 0 {
            Ok(device)
        } else {
            Err(format!(
                "AudioObjectGetPropertyData(default input) status={status}"
            ))
        }
    }

    fn default_input_transport_type() -> Result<u32, String> {
        let device = default_input_device()?;
        let address = AudioObjectPropertyAddress {
            m_selector: K_AUDIO_DEVICE_PROPERTY_TRANSPORT_TYPE,
            m_scope: K_AUDIO_OBJECT_PROPERTY_SCOPE_GLOBAL,
            m_element: K_AUDIO_OBJECT_PROPERTY_ELEMENT_MAIN,
        };
        if unsafe { AudioObjectHasProperty(device, &address) } == 0 {
            return Err("default input device has no transport type property".into());
        }

        let mut transport: u32 = 0;
        let mut size = std::mem::size_of::<u32>() as u32;
        let status = unsafe {
            AudioObjectGetPropertyData(
                device,
                &address,
                0,
                std::ptr::null(),
                &mut size,
                (&mut transport as *mut u32).cast::<c_void>(),
            )
        };
        if status == 0 {
            Ok(transport)
        } else {
            Err(format!(
                "AudioObjectGetPropertyData(input transport) status={status}"
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{input_name_looks_bluetooth, input_name_looks_local_mic, input_name_looks_virtual};

    #[test]
    fn detects_common_bluetooth_headset_inputs() {
        assert!(input_name_looks_bluetooth("Abhishek's AirPods Pro"));
        assert!(input_name_looks_bluetooth("WH-1000XM5"));
        assert!(input_name_looks_bluetooth("Bose QuietComfort Ultra"));
        assert!(input_name_looks_bluetooth("Galaxy Buds2 Pro"));
        assert!(input_name_looks_bluetooth("Bluetooth Headset"));
    }

    #[test]
    fn keeps_local_mics_as_safe_fallbacks() {
        assert!(input_name_looks_local_mic("MacBook Pro Microphone"));
        assert!(input_name_looks_local_mic("Built-in Microphone"));
        assert!(input_name_looks_local_mic("Studio Display Microphone"));
        assert!(!input_name_looks_bluetooth("MacBook Pro Microphone"));
    }

    #[test]
    fn filters_virtual_inputs_from_bluetooth_fallbacks() {
        assert!(input_name_looks_virtual("BlackHole 2ch"));
        assert!(input_name_looks_virtual("Loopback Audio"));
        assert!(input_name_looks_virtual("Soundflower (2ch)"));
    }
}
