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

            // Push the post-conversion samples through the same fan-out that the
            // F32-only path used to do inline. Kept as a closure so the match
            // arms below stay short.
            fn fan_out(
                mono: Vec<f32>,
                frames_cb: &Arc<Mutex<Vec<f32>>>,
                chunk_tx_cb: &mpsc::SyncSender<Vec<f32>>,
                level_tx_cb: &mpsc::SyncSender<f32>,
            ) {
                if mono.is_empty() {
                    return;
                }
                frames_cb.lock().unwrap().extend_from_slice(&mono);
                let _ = chunk_tx_cb.try_send(mono.clone());
                let sum_sq = mono.iter().map(|s| s * s).sum::<f32>();
                let rms = (sum_sq / mono.len() as f32).sqrt();
                let boosted = (rms * 9.0).clamp(0.0, 1.0);
                let _ = level_tx_cb.try_send(boosted);
            }

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
                            fan_out(mono, &frames_cb, &chunk_tx_cb, &level_tx_cb);
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
                            fan_out(mono, &frames_cb, &chunk_tx_cb, &level_tx_cb);
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
                            fan_out(mono, &frames_cb, &chunk_tx_cb, &level_tx_cb);
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

impl Default for AudioRecorder {
    fn default() -> Self {
        Self::new()
    }
}
