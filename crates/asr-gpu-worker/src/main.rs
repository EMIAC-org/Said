//! Isolated GPU transcription worker (Windows/Linux, Vulkan).
//!
//! Spawned by the desktop app as a sidecar. Speaks the [`asr_core::ipc`] protocol
//! over stdin/stdout (STDOUT is the binary channel; ALL logging goes to STDERR).
//!
//! Lifecycle:
//!   1. Probe Vulkan and pick the best device (prefer discrete).
//!      * No usable GPU / no loader → emit `NoGpu` and exit 0. The app then runs
//!        CPU-only and won't respawn this session.
//!   2. Emit `Ready { device }` and load the model warm on request.
//!   3. Serve `Transcribe` / `Prewarm` until stdin closes or `Shutdown`.
//!
//! If ggml aborts the process on a bad GPU/driver (`GGML_ABORT` / `exit(1)`),
//! only *this* process dies — the app detects the exit and fails over to CPU.

use std::io::{BufReader, BufWriter, ErrorKind};
use std::path::Path;

use asr_core::ipc::{self, FromWorker, ToWorker};
use asr_core::{Device, LocalAsrOutput, WhisperEngine};

fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let mut out = BufWriter::new(std::io::stdout().lock());

    // 0. Pre-AVX2 CPUs can't run this ggml build at all (illegal instruction on
    // the first call — even GPU inference runs ggml CPU code for mel/load).
    // Refuse cleanly so the app degrades to cloud STT instead of losing us.
    if !asr_core::cpu_supports_local_asr() {
        tracing::info!("[asr-gpu-worker] CPU lacks AVX2; exiting for cloud fallback");
        let _ = ipc::write_message(
            &mut out,
            &FromWorker::NoGpu {
                reason: "CPU lacks AVX2 (required by the on-device model)".to_string(),
            },
        );
        return;
    }

    // 1. Select a device. Crash-safe: any loader/driver problem yields None.
    let device = match asr_core::probe::select_best_gpu() {
        Some(d) => d,
        None => {
            tracing::info!("[asr-gpu-worker] no usable Vulkan GPU; exiting for CPU fallback");
            let _ = ipc::write_message(
                &mut out,
                &FromWorker::NoGpu {
                    reason: "no usable Vulkan GPU found".to_string(),
                },
            );
            return;
        }
    };
    tracing::info!(
        name = %device.name,
        index = device.index,
        vram_mb = device.vram_mb,
        discrete = device.discrete,
        "[asr-gpu-worker] selected GPU"
    );

    let mut engine = WhisperEngine::new(Device::Gpu(device.index));
    if ipc::write_message(&mut out, &FromWorker::Ready { device }).is_err() {
        return; // app went away before we finished handshaking
    }

    // 3. Serve requests.
    let mut input = BufReader::new(std::io::stdin().lock());
    loop {
        let msg: ToWorker = match ipc::read_message(&mut input) {
            Ok(m) => m,
            Err(e) if e.kind() == ErrorKind::UnexpectedEof => {
                tracing::info!("[asr-gpu-worker] stdin closed; shutting down");
                break;
            }
            Err(e) => {
                tracing::error!(error = %e, "[asr-gpu-worker] IPC read error; shutting down");
                break;
            }
        };

        match msg {
            ToWorker::Shutdown => break,

            ToWorker::Prewarm { cfg } => match engine.ensure_loaded(&cfg) {
                Ok(load_ms) => {
                    tracing::info!(load_ms, "[asr-gpu-worker] warm model ready");
                    if ipc::write_message(&mut out, &FromWorker::Prewarmed { load_ms }).is_err() {
                        break;
                    }
                }
                Err(e) => tracing::warn!(error = %e, "[asr-gpu-worker] prewarm failed"),
            },

            ToWorker::Transcribe { id, pcm, cfg } => {
                let model = model_label(&cfg.model);
                let language = cfg.language.clone();
                let reply = match engine.transcribe(&pcm, &cfg) {
                    Ok(t) => FromWorker::Done {
                        id,
                        output: LocalAsrOutput {
                            transcript: t.text,
                            model,
                            language,
                            total_ms: 0, // filled by the app (spans IPC)
                            load_ms: t.load_ms,
                            inference_ms: t.inference_ms,
                            queue_wait_ms: 0,
                        },
                    },
                    Err(error) => {
                        tracing::warn!(id, error = %error, "[asr-gpu-worker] transcription failed");
                        FromWorker::Failed { id, error }
                    }
                };
                if ipc::write_message(&mut out, &reply).is_err() {
                    break;
                }
            }
        }
    }
}

fn model_label(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("local-whisper")
        .to_string()
}
