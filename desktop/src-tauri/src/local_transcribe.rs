//! Generic transcribe.cpp runtime for catalog-backed local speech models.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use transcribe_cpp::{Model, RunOptions, StreamOptions};

use crate::local_model_catalog::{self, LocalModelDescriptor};
use crate::local_model_store;

const STREAM_QUEUE_DEPTH: usize = 64;
const STREAM_FINALIZE_TIMEOUT: Duration = Duration::from_secs(30);
const IDLE_UNLOAD_AFTER: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone)]
pub struct Output {
    pub transcript: String,
    pub language: Option<String>,
    pub duration_ms: u64,
    pub streamed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeStatus {
    pub selected_model: Option<String>,
    pub loaded_model: Option<String>,
    pub backend: Option<String>,
    pub architecture: Option<String>,
    pub streaming: bool,
    pub loading: bool,
    pub supports_streaming: Option<bool>,
    pub last_load_ms: Option<u64>,
    pub last_error: Option<String>,
}

struct CachedModel {
    key: String,
    model: Model,
}

enum StreamCommand {
    Feed(Vec<f32>),
    Finalize(mpsc::Sender<Option<Output>>),
    Cancel,
}

struct ActiveStream {
    recording_id: String,
    sender: SyncSender<StreamCommand>,
    overflowed: Arc<AtomicBool>,
    released: Receiver<()>,
}

static MODEL: OnceLock<Mutex<Option<CachedModel>>> = OnceLock::new();
static STREAM: OnceLock<Mutex<Option<ActiveStream>>> = OnceLock::new();
static LAST_ACTIVITY_SECS: AtomicU64 = AtomicU64::new(0);
static STREAM_GENERATION: AtomicU64 = AtomicU64::new(0);
static LOADING: AtomicBool = AtomicBool::new(false);
static LAST_LOAD_MS: AtomicU64 = AtomicU64::new(0);
static LAST_ERROR: OnceLock<Mutex<Option<String>>> = OnceLock::new();
static IDLE_WATCHER: OnceLock<()> = OnceLock::new();

fn model_cache() -> &'static Mutex<Option<CachedModel>> {
    MODEL.get_or_init(|| Mutex::new(None))
}

fn active_stream() -> &'static Mutex<Option<ActiveStream>> {
    STREAM.get_or_init(|| Mutex::new(None))
}

fn last_error() -> &'static Mutex<Option<String>> {
    LAST_ERROR.get_or_init(|| Mutex::new(None))
}

struct LoadingGuard;

impl LoadingGuard {
    fn begin() -> Self {
        LOADING.store(true, Ordering::Release);
        Self
    }
}

impl Drop for LoadingGuard {
    fn drop(&mut self) {
        LOADING.store(false, Ordering::Release);
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn touch_activity() {
    LAST_ACTIVITY_SECS.store(now_secs(), Ordering::Release);
}

fn ensure_idle_watcher() {
    IDLE_WATCHER.get_or_init(|| {
        std::thread::Builder::new()
            .name("local-asr-idle-unload".into())
            .spawn(|| {
                loop {
                    std::thread::sleep(Duration::from_secs(60));
                    let last = LAST_ACTIVITY_SECS.load(Ordering::Acquire);
                    if last == 0 || now_secs().saturating_sub(last) < IDLE_UNLOAD_AFTER.as_secs() {
                        continue;
                    }
                    let streaming = active_stream()
                        .lock()
                        .map(|stream| stream.is_some())
                        .unwrap_or(true);
                    if !streaming {
                        unload();
                    }
                }
            })
            .expect("local ASR idle watcher should start");
    });
}

pub fn selected_descriptor() -> Option<&'static LocalModelDescriptor> {
    let prefs = said_core::prefs::load();
    local_model_catalog::find(&prefs.local_stt_model)
}

pub fn is_selected() -> bool {
    selected_descriptor().is_some()
}

pub fn selected_installed() -> bool {
    selected_descriptor().is_some_and(local_model_store::installed)
}

pub fn selected_supports_streaming() -> bool {
    selected_descriptor().is_some_and(|model| model.capabilities.streaming)
}

pub fn selected_model_file() -> &'static str {
    selected_descriptor().map_or("", |model| model.filename)
}

pub fn selected_model_name() -> &'static str {
    selected_descriptor().map_or("Local speech model", |model| model.name)
}

pub fn unload() {
    cancel_stream();
    if let Ok(mut cache) = model_cache().lock()
        && cache.take().is_some()
    {
        tracing::info!("[local-asr] unloaded idle or replaced model");
    }
    LAST_ACTIVITY_SECS.store(0, Ordering::Release);
}

fn loaded_model(descriptor: &LocalModelDescriptor) -> Result<Model, String> {
    ensure_idle_watcher();
    touch_activity();
    let path = local_model_store::ensure_verified(descriptor)?;
    let mut cache = model_cache()
        .lock()
        .map_err(|_| "Local ASR model cache is unavailable".to_string())?;
    if let Some(cached) = cache.as_ref()
        && cached.key == descriptor.key
    {
        return Ok(cached.model.clone());
    }
    // Never map two large models at once.
    *cache = None;
    let started = Instant::now();
    let _loading = LoadingGuard::begin();
    let model = Model::load(&path).map_err(|error| {
        let message = format!("Couldn't load {}: {error}", descriptor.name);
        if let Ok(mut last) = last_error().lock() {
            *last = Some(message.clone());
        }
        message
    })?;
    let load_ms = started.elapsed().as_millis() as u64;
    LAST_LOAD_MS.store(load_ms, Ordering::Release);
    if let Ok(mut last) = last_error().lock() {
        *last = None;
    }
    let capabilities = model.capabilities();
    tracing::info!(
        model = descriptor.key,
        backend = %model.backend(),
        architecture = %model.arch(),
        load_ms,
        streaming = capabilities.supports_streaming,
        "[local-asr] model loaded"
    );
    *cache = Some(CachedModel {
        key: descriptor.key.to_string(),
        model: model.clone(),
    });
    Ok(model)
}

pub fn prewarm() {
    let Some(descriptor) = selected_descriptor() else {
        return;
    };
    if !local_model_store::installed(descriptor) {
        return;
    }
    if let Err(error) = loaded_model(descriptor) {
        tracing::warn!(model = descriptor.key, %error, "[local-asr] prewarm failed");
    }
}

#[tauri::command]
pub fn local_asr_runtime_status() -> RuntimeStatus {
    let selected_model = selected_descriptor().map(|model| model.key.to_string());
    let (loaded_model, backend, architecture, supports_streaming) = model_cache()
        .lock()
        .ok()
        .and_then(|cache| {
            cache.as_ref().map(|cached| {
                (
                    Some(cached.key.clone()),
                    Some(cached.model.backend()),
                    Some(cached.model.arch()),
                    Some(cached.model.capabilities().supports_streaming),
                )
            })
        })
        .unwrap_or((None, None, None, None));
    let streaming = active_stream()
        .lock()
        .map(|stream| stream.is_some())
        .unwrap_or(false);
    RuntimeStatus {
        selected_model,
        loaded_model,
        backend,
        architecture,
        streaming,
        loading: LOADING.load(Ordering::Acquire),
        supports_streaming,
        last_load_ms: match LAST_LOAD_MS.load(Ordering::Acquire) {
            0 => None,
            value => Some(value),
        },
        last_error: last_error().lock().ok().and_then(|error| error.clone()),
    }
}

pub fn transcribe_wav_bytes(
    wav: &[u8],
    requested_language: &str,
    recording_id: Option<&str>,
) -> Result<Output, String> {
    let descriptor =
        selected_descriptor().ok_or_else(|| "No catalog local model is selected.".to_string())?;
    transcribe_wav_bytes_for(descriptor, wav, requested_language, recording_id)
}

pub fn transcribe_wav_bytes_for_key(
    model: &str,
    wav: &[u8],
    requested_language: &str,
) -> Result<Output, String> {
    let descriptor = local_model_catalog::find(model)
        .ok_or_else(|| format!("Unknown local speech model: {model}"))?;
    transcribe_wav_bytes_for(descriptor, wav, requested_language, None)
}

fn transcribe_wav_bytes_for(
    descriptor: &LocalModelDescriptor,
    wav: &[u8],
    requested_language: &str,
    recording_id: Option<&str>,
) -> Result<Output, String> {
    let effective_language = effective_language(descriptor, requested_language);
    if !descriptor.supports_language(effective_language) {
        return Err(format!(
            "{} supports English only. Choose English or select a multilingual speech model.",
            descriptor.name
        ));
    }
    if let Some(recording_id) = recording_id
        && let Some(streamed) = finalize_stream(recording_id)?
    {
        return Ok(streamed);
    }
    let started = Instant::now();
    // Raw 16 kHz capture, not whisper-conditioned audio: these are conformer /
    // TDT models trained on unprocessed speech.
    let pcm = asr_core::audio::decode_16k(wav).map_err(|error| error.to_string())?;
    let model = loaded_model(descriptor)?;
    let mut session = model
        .session()
        .map_err(|error| format!("Couldn't start {}: {error}", descriptor.name))?;
    let options = RunOptions {
        language: language_hint(&model.capabilities().languages, effective_language),
        ..RunOptions::default()
    };
    let run =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| session.run(&pcm, &options)));
    let result = match run {
        Ok(result) => {
            result.map_err(|error| format!("{} transcription failed: {error}", descriptor.name))?
        }
        Err(_) => {
            unload();
            return Err(format!(
                "{} crashed during transcription and was unloaded. Try again.",
                descriptor.name
            ));
        }
    };
    touch_activity();
    let raw_text = result.text.clone();
    let transcript = strip_terminal_language_tag(result.text).trim().to_string();
    if transcript.is_empty() {
        // "No speech detected" is the recorder's verdict, not the model's. A
        // model that ran on real audio and decoded nothing is a different
        // failure, and reporting it as silence hides every clue needed to
        // diagnose it — so log what the runtime actually produced and say so.
        let levels = pcm_levels(&pcm);
        tracing::warn!(
            model = descriptor.key,
            language = options.language.as_deref().unwrap_or("auto"),
            detected_language = result.language.as_deref().unwrap_or("unreported"),
            samples = pcm.len(),
            peak = levels.0,
            rms = levels.1,
            segments = result.segments.len(),
            tokens = result.tokens.len(),
            raw_chars = raw_text.chars().count(),
            "[local-asr] model decoded no text from audible input"
        );
        return Err(format!(
            "{} returned an empty transcript for this recording. Try again, or switch speech model in Settings → Speech recognition.",
            descriptor.name
        ));
    }
    Ok(Output {
        transcript,
        language: result.language,
        duration_ms: started.elapsed().as_millis() as u64,
        streamed: false,
    })
}

/// Peak and RMS of the buffer actually handed to the model, so an empty decode
/// can be separated from an empty microphone in one log line.
fn pcm_levels(pcm: &[f32]) -> (f32, f32) {
    if pcm.is_empty() {
        return (0.0, 0.0);
    }
    let peak = pcm
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
    let sum_sq: f64 = pcm
        .iter()
        .map(|sample| (*sample as f64) * (*sample as f64))
        .sum();
    (peak, (sum_sq / pcm.len() as f64).sqrt() as f32)
}

fn effective_language<'a>(
    descriptor: &LocalModelDescriptor,
    requested_language: &'a str,
) -> &'a str {
    if descriptor.languages == ["en"] {
        "english"
    } else {
        requested_language
    }
}

pub fn start_stream(app: AppHandle, recording_id: String) {
    cancel_stream();
    let Some(descriptor) = selected_descriptor() else {
        return;
    };
    if !descriptor.capabilities.streaming || !local_model_store::installed(descriptor) {
        return;
    }
    let generation = STREAM_GENERATION.fetch_add(1, Ordering::AcqRel) + 1;
    let (sender, receiver) = mpsc::sync_channel(STREAM_QUEUE_DEPTH);
    let (released_tx, released_rx) = mpsc::channel();
    let overflowed = Arc::new(AtomicBool::new(false));
    if let Ok(mut active) = active_stream().lock() {
        *active = Some(ActiveStream {
            recording_id: recording_id.clone(),
            sender,
            overflowed: Arc::clone(&overflowed),
            released: released_rx,
        });
    } else {
        return;
    }
    std::thread::Builder::new()
        .name("local-asr-stream".into())
        .spawn(move || {
            run_stream_worker(
                app,
                descriptor,
                recording_id,
                receiver,
                overflowed,
                generation,
            );
            let _ = released_tx.send(());
        })
        .ok();
}

pub fn feed_stream(recording_id: &str, pcm_16khz: Vec<f32>) {
    let Ok(active) = active_stream().lock() else {
        return;
    };
    let Some(stream) = active.as_ref() else {
        return;
    };
    if stream.recording_id != recording_id {
        return;
    }
    match stream.sender.try_send(StreamCommand::Feed(pcm_16khz)) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) => {
            stream.overflowed.store(true, Ordering::Release);
        }
        Err(TrySendError::Disconnected(_)) => {}
    }
}

fn take_stream_for(active: &mut Option<ActiveStream>, recording_id: &str) -> Option<ActiveStream> {
    if active
        .as_ref()
        .is_some_and(|stream| stream.recording_id == recording_id)
    {
        active.take()
    } else {
        None
    }
}

pub fn finalize_stream(recording_id: &str) -> Result<Option<Output>, String> {
    crate::whisper_dictation_stream::wait_for_drain(recording_id, Duration::from_secs(2));
    let stream = {
        let mut active = active_stream()
            .lock()
            .map_err(|_| "Local ASR stream registry is unavailable".to_string())?;
        let Some(stream) = take_stream_for(&mut active, recording_id) else {
            return Ok(None);
        };
        Some(stream)
    };
    let Some(stream) = stream else {
        return Ok(None);
    };
    if stream.overflowed.load(Ordering::Acquire) {
        let _ = stream.sender.send(StreamCommand::Cancel);
        tracing::warn!("[local-asr] live queue overflowed; using complete-audio batch fallback");
        return wait_for_stream_release(stream, recording_id);
    }
    let (reply_tx, reply_rx) = mpsc::channel();
    if stream
        .sender
        .send(StreamCommand::Finalize(reply_tx))
        .is_err()
    {
        return Ok(None);
    }
    match reply_rx.recv_timeout(STREAM_FINALIZE_TIMEOUT) {
        Ok(result) => Ok(result),
        Err(mpsc::RecvTimeoutError::Disconnected) => wait_for_stream_release(stream, recording_id),
        Err(mpsc::RecvTimeoutError::Timeout) => Err(format!(
            "Timed out releasing the live local transcription session for recording {recording_id}."
        )),
    }
}

fn wait_for_stream_release(
    stream: ActiveStream,
    recording_id: &str,
) -> Result<Option<Output>, String> {
    stream
        .released
        .recv_timeout(STREAM_FINALIZE_TIMEOUT)
        .map(|_| None)
        .map_err(|_| {
            format!(
                "Timed out releasing the live local transcription session for recording {recording_id}."
            )
        })
}

pub fn cancel_stream() {
    STREAM_GENERATION.fetch_add(1, Ordering::AcqRel);
    if let Ok(mut active) = active_stream().lock()
        && let Some(stream) = active.take()
    {
        let _ = stream.sender.try_send(StreamCommand::Cancel);
    }
}

fn run_stream_worker(
    app: AppHandle,
    descriptor: &'static LocalModelDescriptor,
    recording_id: String,
    receiver: Receiver<StreamCommand>,
    overflowed: Arc<AtomicBool>,
    generation: u64,
) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_stream_worker_inner(
            &app,
            descriptor,
            &recording_id,
            receiver,
            &overflowed,
            generation,
        )
    }));
    if result.is_err() && STREAM_GENERATION.load(Ordering::Acquire) == generation {
        unload();
        let _ = app.emit(
            "local-stt-stream-error",
            serde_json::json!({
                "model": descriptor.key,
                "recording_id": recording_id,
                "message": "The local streaming engine crashed and was unloaded. Batch retry will be used."
            }),
        );
    }
}

fn run_stream_worker_inner(
    app: &AppHandle,
    descriptor: &'static LocalModelDescriptor,
    recording_id: &str,
    receiver: Receiver<StreamCommand>,
    overflowed: &AtomicBool,
    generation: u64,
) {
    let Ok(model) = loaded_model(descriptor) else {
        drain_stream(receiver);
        return;
    };
    let capabilities = model.capabilities();
    if !capabilities.supports_streaming {
        tracing::info!(
            model = descriptor.key,
            "[local-asr] runtime reports no streaming support"
        );
        drain_stream(receiver);
        return;
    }
    let Ok(mut session) = model.session() else {
        drain_stream(receiver);
        return;
    };
    let options = RunOptions {
        // English-only models get an explicit hint; everything else auto-detects,
        // matching the batch path.
        language: language_hint(
            &capabilities.languages,
            effective_language(descriptor, "auto"),
        ),
        ..RunOptions::default()
    };
    let Ok(mut stream) = session.stream(&options, &StreamOptions::default()) else {
        drain_stream(receiver);
        return;
    };
    while let Ok(command) = receiver.recv() {
        match command {
            StreamCommand::Feed(pcm) => {
                if overflowed.load(Ordering::Acquire)
                    || STREAM_GENERATION.load(Ordering::Acquire) != generation
                {
                    stream.reset();
                    drain_stream(receiver);
                    return;
                }
                match stream.feed(&pcm) {
                    Ok(update) if update.committed_changed || update.tentative_changed => {
                        if STREAM_GENERATION.load(Ordering::Acquire) != generation {
                            stream.reset();
                            return;
                        }
                        let text = stream.text();
                        let _ = app.emit(
                            "local-stt-partial",
                            serde_json::json!({
                                "model": descriptor.key,
                                "recording_id": recording_id,
                                "committed": text.committed,
                                "tentative": text.tentative,
                                "text": text.display(),
                            }),
                        );
                    }
                    Ok(_) => {}
                    Err(error) => {
                        tracing::warn!(model = descriptor.key, %error, "[local-asr] stream feed failed");
                        stream.reset();
                        drain_stream(receiver);
                        return;
                    }
                }
            }
            StreamCommand::Finalize(reply) => {
                // transcribe.cpp holds a per-model compute lease for a live
                // stream. Release it before acknowledging finalization so the
                // complete-WAV batch pass cannot race into `Busy`.
                drop(stream);
                drop(session);
                touch_activity();
                let _ = reply.send(None);
                return;
            }
            StreamCommand::Cancel => {
                stream.reset();
                return;
            }
        }
    }
}

fn drain_stream(receiver: Receiver<StreamCommand>) {
    while let Ok(command) = receiver.recv() {
        match command {
            StreamCommand::Feed(_) => {}
            StreamCommand::Finalize(reply) => {
                let _ = reply.send(None);
                return;
            }
            StreamCommand::Cancel => return,
        }
    }
}

/// Resolve the user's language intent to a hint the *loaded* model advertises.
///
/// The loaded GGUF is the authority on which codes it accepts, not our catalog:
/// a code the model does not list is rejected outright with
/// `UNSUPPORTED_LANGUAGE`, so an unmatched intent falls back to auto-detect.
fn language_hint(advertised: &[String], requested_language: &str) -> Option<String> {
    let requested = match requested_language.trim().to_ascii_lowercase().as_str() {
        "en" | "english" => "en",
        "hi" | "hindi" => "hi",
        _ => return None,
    };
    advertised
        .iter()
        .find(|code| code.split(['-', '_']).next() == Some(requested))
        .cloned()
}

fn strip_terminal_language_tag(text: String) -> String {
    let trimmed = text.trim_end();
    let Some(start) = trimmed.rfind('<') else {
        return trimmed.to_string();
    };
    let candidate = &trimmed[start..];
    let is_language_tag = candidate.len() == 7
        && candidate.starts_with('<')
        && candidate.ends_with('>')
        && candidate.as_bytes()[3] == b'-';
    if is_language_tag {
        trimmed[..start].trim_end().to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn advertised(codes: &[&str]) -> Vec<String> {
        codes.iter().map(|code| (*code).to_string()).collect()
    }

    #[test]
    fn hints_only_languages_the_loaded_model_advertises() {
        // The model's own code wins, in its own spelling — a hint it does not
        // list is rejected with UNSUPPORTED_LANGUAGE, so fall back to auto.
        assert_eq!(
            language_hint(&advertised(&["en", "hi"]), "english"),
            Some("en".to_string())
        );
        assert_eq!(
            language_hint(&advertised(&["en-US", "hi-IN"]), "hindi"),
            Some("hi-IN".to_string())
        );
        assert_eq!(language_hint(&advertised(&["en"]), "hindi"), None);
        assert_eq!(language_hint(&advertised(&["en"]), "auto"), None);
        // Language-agnostic models advertise nothing and must stay on auto.
        assert_eq!(language_hint(&advertised(&[]), "english"), None);
    }

    #[test]
    fn english_only_models_coerce_a_mixed_language_request() {
        let parakeet = local_model_catalog::find(local_model_catalog::PARAKEET_Q8_PREF).unwrap();
        assert_eq!(effective_language(parakeet, "hinglish"), "english");
    }

    #[test]
    fn removes_only_terminal_language_tag() {
        assert_eq!(
            strip_terminal_language_tag("Hello. <en-US>".into()),
            "Hello."
        );
        assert_eq!(
            strip_terminal_language_tag("hello <not-a-tag>".into()),
            "hello <not-a-tag>"
        );
    }

    /// End-to-end check against a real installed model and a real recording.
    ///
    /// Ignored by default: it needs a downloaded GGUF and a WAV on this
    /// machine. Run it when changing run options or audio handling — a
    /// rejected language or a broken audio path fails here instead of in the
    /// user's dictation.
    ///
    ///   AIRNOTE_TEST_MODEL=parakeet-en-q8 AIRNOTE_TEST_WAV=/path/to.wav \
    ///     cargo test -p said-desktop transcribes_installed_model -- --ignored --nocapture
    #[test]
    #[ignore = "requires a locally installed model and a recording"]
    fn transcribes_installed_model_end_to_end() {
        let key = std::env::var("AIRNOTE_TEST_MODEL").expect("AIRNOTE_TEST_MODEL");
        let wav_path = std::env::var("AIRNOTE_TEST_WAV").expect("AIRNOTE_TEST_WAV");
        let wav = std::fs::read(&wav_path).expect("read AIRNOTE_TEST_WAV");
        let descriptor = local_model_catalog::find(&key).expect("model is in the catalog");
        let language = std::env::var("AIRNOTE_TEST_LANGUAGE").unwrap_or_else(|_| "hi".into());

        let output = transcribe_wav_bytes_for(descriptor, &wav, &language, None);
        // Release the model before the harness exits. ggml's Metal device is
        // freed by a C++ static destructor that asserts its residency sets are
        // empty; a still-mapped model aborts the process after the test has
        // already passed. The app sets GGML_METAL_NO_RESIDENCY and _exit()s
        // instead, so only the test harness reaches that destructor.
        unload();
        let output = output.unwrap_or_else(|error| panic!("{} failed: {error}", descriptor.name));

        println!(
            "{} -> {:?} ({} ms, language {:?})",
            descriptor.name, output.transcript, output.duration_ms, output.language
        );
        assert!(!output.transcript.trim().is_empty());
    }

    #[test]
    fn older_recording_cannot_take_the_newer_recordings_stream() {
        let (sender, _receiver) = mpsc::sync_channel(1);
        let (_released_tx, released) = mpsc::channel();
        let mut active = Some(ActiveStream {
            recording_id: "recording-b".into(),
            sender,
            overflowed: Arc::new(AtomicBool::new(false)),
            released,
        });

        assert!(take_stream_for(&mut active, "recording-a").is_none());
        assert_eq!(
            active.as_ref().map(|stream| stream.recording_id.as_str()),
            Some("recording-b")
        );
        assert!(take_stream_for(&mut active, "recording-b").is_some());
        assert!(active.is_none());
    }
}
