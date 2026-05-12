use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::io::Cursor;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

// ── Recording constants ───────────────────────────────────────────────────────

pub const SAMPLE_RATE: u32 = 16_000;
pub const CHANNELS: u16 = 1;
pub const MIN_DURATION_S: f32 = 0.5;

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

// ── Chunk receiver ────────────────────────────────────────────────────────────

/// A live handle to raw audio chunks as they arrive from the microphone.
/// Used by the Deepgram WebSocket streaming pipeline (P5).
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
        let host = cpal::default_host();
        let _device = host.default_input_device().ok_or("no input device found")?;

        let frames: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
        let frames_for_reply = Arc::clone(&frames);

        let (cmd_tx, cmd_rx) = mpsc::channel::<RecCmd>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<u32, String>>();

        // Chunk channel for WS streaming (P5): buffer 256 cpal frames
        let (chunk_tx, chunk_rx) = mpsc::sync_channel::<Vec<f32>>(256);
        let chunk_tx_cb = chunk_tx.clone(); // moved into cpal callback
        self.chunk_tx = Some(chunk_tx); // dropped in stop() to close the channel
        self.chunk_rx = Some(chunk_rx);

        let (level_tx, level_rx) = mpsc::sync_channel::<f32>(64);
        let level_tx_cb = level_tx.clone();
        self.level_tx = Some(level_tx);
        self.level_rx = Some(level_rx);

        std::thread::spawn(move || {
            let host = cpal::default_host();
            let device = match host.default_input_device() {
                Some(d) => d,
                None => {
                    let _ = ready_tx.send(Err("no input device found".into()));
                    return;
                }
            };

            let default_config = match device.default_input_config() {
                Ok(c) => c,
                Err(e) => {
                    let _ = ready_tx.send(Err(format!("no default input config: {e}")));
                    return;
                }
            };

            let native_rate = default_config.sample_rate().0;
            let config = cpal::StreamConfig {
                channels: CHANNELS,
                sample_rate: cpal::SampleRate(native_rate),
                buffer_size: cpal::BufferSize::Default,
            };

            let frames_cb = Arc::clone(&frames_for_reply);
            let stream = match device.build_input_stream(
                &config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    frames_cb.lock().unwrap().extend_from_slice(data);
                    // Non-blocking send to WS pipeline; drop chunk on back-pressure
                    let _ = chunk_tx_cb.try_send(data.to_vec());
                    if !data.is_empty() {
                        let sum_sq = data.iter().map(|s| s * s).sum::<f32>();
                        let rms = (sum_sq / data.len() as f32).sqrt();
                        let boosted = (rms * 9.0).clamp(0.0, 1.0);
                        let _ = level_tx_cb.try_send(boosted);
                    }
                },
                |err| eprintln!("[rec] stream error: {err}"),
                None,
            ) {
                Ok(s) => s,
                Err(e) => {
                    let _ = ready_tx.send(Err(format!("failed to open audio stream: {e}")));
                    return;
                }
            };

            if let Err(e) = stream.play() {
                let _ = ready_tx.send(Err(format!("failed to start stream: {e}")));
                return;
            }

            let _ = ready_tx.send(Ok(native_rate));

            if let Ok(RecCmd::Stop(reply)) = cmd_rx.recv() {
                // `stream` drops here → chunk_tx_cb drops → all senders gone → chunk_rx sees close
                drop(stream);
                let data = frames_for_reply.lock().unwrap().clone();
                let _ = reply.send((data, native_rate));
            }
        });

        match ready_rx.recv() {
            Ok(Ok(rate)) => {
                self.native_rate = Some(rate);
                println!("[rec] opened at {rate}Hz F32");
            }
            Ok(Err(e)) => return Err(e),
            Err(_) => return Err("recording thread died".into()),
        }

        self.cmd_tx = Some(cmd_tx);
        println!("[rec] recording … press hotkey again to stop");
        Ok(())
    }

    /// Take the chunk receiver for the Deepgram WS streaming pipeline.
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

    pub fn collect_wav(reply_rx: StopReceiver) -> Option<Vec<u8>> {
        let (samples_f32, native_rate) = reply_rx.recv().ok()?;

        if samples_f32.is_empty() {
            println!("[rec] no audio captured");
            return None;
        }

        let duration = samples_f32.len() as f32 / native_rate as f32;
        let max_amp = samples_f32.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        println!("[rec] {duration:.1}s recorded ({native_rate}Hz → 16kHz, peak={max_amp:.4})");

        if max_amp < 0.0001 {
            eprintln!("[rec] audio is silence — microphone permission not granted?");
            eprintln!("[rec]   System Settings → Privacy & Security → Microphone");
            return None;
        }

        if duration < MIN_DURATION_S {
            println!("[rec] too short — ignored");
            return None;
        }

        // ── P1: Resample to 16 kHz (2.75× smaller WAV → faster Deepgram upload) ──
        let resampled = resample_to_16k(&samples_f32, native_rate);

        // Convert F32 → I16 WAV at 16 kHz
        let mut buf = Cursor::new(Vec::new());
        let spec = hound::WavSpec {
            channels: CHANNELS,
            sample_rate: SAMPLE_RATE, // 16_000 Hz
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::new(&mut buf, spec).ok()?;
        for &sample in &resampled {
            let clamped = sample.clamp(-1.0, 1.0);
            writer.write_sample((clamped * 32767.0) as i16).ok()?;
        }
        writer.finalize().ok()?;

        Some(buf.into_inner())
    }

    pub fn stop(&mut self) -> Option<Vec<u8>> {
        let reply_rx = self.initiate_stop()?;
        Self::collect_wav(reply_rx)
    }

    pub fn preflight() -> Result<String, String> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or("no input device found — check microphone connection")?;
        let name = device.name().unwrap_or_else(|_| "unknown".into());
        Ok(name)
    }
}

// ── Device enumeration ────────────────────────────────────────────────────────

/// Cross-platform description of a single input device. The `id` is the
/// stable string the UI passes back to select this device.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

/// List every input device cpal can see. On Windows this includes WASAPI
/// shared-mode devices plus any virtual mics (Stereo Mix, NVIDIA Broadcast,
/// Krisp). On macOS it's the AVAudioEngine input devices.
///
/// Returns an empty vec if the host has no input device API at all (cpal
/// errors are converted to empty lists since the UI just needs a picker).
pub fn list_input_devices() -> Vec<DeviceInfo> {
    let host = cpal::default_host();
    let default_name = host
        .default_input_device()
        .and_then(|d| d.name().ok())
        .unwrap_or_default();

    let Ok(devices) = host.input_devices() else {
        return vec![];
    };

    devices
        .filter_map(|d| {
            let name = d.name().ok()?;
            Some(DeviceInfo {
                id: name.clone(),
                is_default: name == default_name,
                name,
            })
        })
        .collect()
}

// ── Microphone permission probe ───────────────────────────────────────────────

/// Result of a permission probe — distinguishable failure modes for UI hints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum MicPermission {
    /// At least one input device is reachable.
    Granted,
    /// OS-level privacy setting blocks microphone access for this app.
    DeniedByPrivacySettings,
    /// No physical / virtual input device present.
    NoDevice,
    /// Other audio-stack failure (driver issue, exclusive-mode contention).
    Unknown,
}

/// Best-effort microphone permission probe. Does NOT open a stream — just
/// asks cpal to enumerate. Sufficient to distinguish denied-by-OS from
/// no-device-attached on Windows (`HRESULT 0x80070005` surfaces as a cpal
/// `BackendSpecific` error during enumeration).
pub fn probe_mic_permission() -> MicPermission {
    let host = cpal::default_host();
    match host.input_devices() {
        Ok(mut devices) => {
            if devices.next().is_some() {
                MicPermission::Granted
            } else {
                MicPermission::NoDevice
            }
        }
        Err(e) => {
            let msg = format!("{e}").to_lowercase();
            // Windows: WASAPI returns `Access is denied. (0x80070005)` when
            // Settings → Privacy → Microphone has disabled desktop apps.
            if msg.contains("access is denied") || msg.contains("0x80070005") {
                MicPermission::DeniedByPrivacySettings
            } else {
                MicPermission::Unknown
            }
        }
    }
}

impl Default for AudioRecorder {
    fn default() -> Self {
        Self::new()
    }
}
