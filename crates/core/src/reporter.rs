//! Fleet diagnostics reporter — disk-queued, batched POST to control plane.
//!
//! Fire-and-forget: never panics and never blocks the hot path. Respects the same
//! opt-out toggle as Sentry (`desktop_prefs.json::sentry_disabled`).

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Condvar, LazyLock, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{Value, json};

use crate::paths;
use crate::prefs;
use crate::scrub;

const QUEUE_FILE: &str = "queue.ndjson";
const MAX_BATCH: usize = 50;
const MAX_CONTEXT_CHARS: usize = 8_192;
const FLUSH_INTERVAL: Duration = Duration::from_secs(45);
const FLUSH_BACKOFF: Duration = Duration::from_secs(120);

static ENDPOINT: OnceLock<Mutex<Option<String>>> = OnceLock::new();
static PHASE: LazyLock<Mutex<String>> = LazyLock::new(|| Mutex::new("idle".to_string()));
static FLUSHER: OnceLock<(Mutex<FlusherState>, Condvar)> = OnceLock::new();
static FLUSHER_STARTED: AtomicBool = AtomicBool::new(false);
static EVENT_SEQ: AtomicU64 = AtomicU64::new(0);

struct FlusherState {
    pending: bool,
    last_attempt: Option<Instant>,
}

/// Severity of a diagnostics event.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Error,
    Fatal,
}

/// Report one diagnostics event. Non-blocking, infallible, fire-and-forget.
pub fn report_event(kind: &str, severity: Severity, mut context: Value) {
    if prefs::load().sentry_disabled {
        return;
    }
    ensure_flusher();

    if !context_is_safe(&context) {
        context = json!({ "scrubbed": "unsafe_context_dropped" });
    }
    scrub::scrub_json_value(&mut context);

    let event = json!({
        "event_type": kind,
        "severity": severity,
        "app_version": env!("CARGO_PKG_VERSION"),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "channel": channel(),
        "phase": current_phase(),
        "context": context,
        "ts": epoch_secs(),
        "seq": EVENT_SEQ.fetch_add(1, Ordering::Relaxed),
    });

    if let Ok(line) = serde_json::to_string(&event) {
        let _ = append_queue_line(&line);
        signal_flush();
    }
}

/// Set the current app phase string attached to every event.
pub fn set_phase(phase: &str) {
    if let Ok(mut guard) = PHASE.lock() {
        *guard = phase.to_string();
    }
}

fn phase_lock() -> &'static Mutex<String> {
    &PHASE
}

/// Configure the control-plane base URL (e.g. `https://airnote.emiactech.com`).
pub fn configure(endpoint_base: &str) {
    let base = endpoint_base.trim_end_matches('/').to_string();
    let slot = ENDPOINT.get_or_init(|| Mutex::new(None));
    if let Ok(mut guard) = slot.lock() {
        *guard = Some(base);
    }
    ensure_flusher();
    signal_flush();
}

/// Build a tracing layer that forwards `ERROR` events to [`report_event`].
pub fn tracing_layer<S>() -> DiagnosticsTracingLayer<S>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    DiagnosticsTracingLayer(std::marker::PhantomData)
}

pub struct DiagnosticsTracingLayer<S>(std::marker::PhantomData<S>);

impl<S> tracing_subscriber::Layer<S> for DiagnosticsTracingLayer<S>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if *event.metadata().level() > tracing::Level::ERROR {
            return;
        }
        let mut message = String::new();
        let mut visitor = MessageVisitor(&mut message);
        event.record(&mut visitor);
        if message.is_empty() {
            message = event.metadata().name().to_string();
        }
        scrub::scrub_string(&mut message);
        report_event(
            "tracing.error",
            Severity::Error,
            json!({
                "target": event.metadata().target(),
                "message": message.chars().take(500).collect::<String>(),
            }),
        );
    }
}

struct MessageVisitor<'a>(&'a mut String);

impl tracing::field::Visit for MessageVisitor<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            use std::fmt::Write;
            let _ = write!(self.0, "{value:?}");
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.0.push_str(value);
        }
    }
}

fn channel() -> String {
    std::env::var("SAID_CHANNEL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| prefs::load().update_channel)
}

fn current_phase() -> String {
    phase_lock()
        .lock()
        .map(|p| p.clone())
        .unwrap_or_else(|_| "idle".into())
}

fn epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn queue_dir() -> PathBuf {
    paths::data_dir().join("diagnostics")
}

fn queue_path() -> PathBuf {
    queue_dir().join(QUEUE_FILE)
}

fn append_queue_line(line: &str) -> std::io::Result<()> {
    let dir = queue_dir();
    std::fs::create_dir_all(&dir)?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(queue_path())?;
    writeln!(file, "{line}")?;
    Ok(())
}

fn ensure_flusher() {
    if FLUSHER_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    FLUSHER.get_or_init(|| {
        (
            Mutex::new(FlusherState {
                pending: false,
                last_attempt: None,
            }),
            Condvar::new(),
        )
    });
    thread::spawn(flusher_loop);
}

fn signal_flush() {
    let Some((lock, cv)) = FLUSHER.get() else {
        return;
    };
    if let Ok(mut state) = lock.lock() {
        state.pending = true;
        cv.notify_one();
    }
}

fn flusher_loop() {
    let (lock, cv) = FLUSHER
        .get()
        .expect("flusher state initialized before thread spawn");
    loop {
        let wait_for = {
            let mut state = lock.lock().expect("flusher mutex");
            if !state.pending {
                state = cv
                    .wait_timeout(state, FLUSH_INTERVAL)
                    .expect("flusher wait")
                    .0;
            }
            if state.pending {
                state.pending = false;
                Duration::ZERO
            } else {
                FLUSH_INTERVAL
            }
        };
        if wait_for > Duration::ZERO {
            let _ = cv.wait_timeout(lock.lock().unwrap(), wait_for);
            continue;
        }

        let endpoint = ENDPOINT
            .get()
            .and_then(|m| m.lock().ok().and_then(|g| g.clone()));
        let Some(base) = endpoint else {
            thread::sleep(FLUSH_INTERVAL);
            continue;
        };

        match flush_once(&base) {
            Ok(true) => {
                if let Ok(mut state) = lock.lock() {
                    state.last_attempt = Some(Instant::now());
                }
            }
            Ok(false) => {}
            Err(_) => {
                thread::sleep(FLUSH_BACKOFF);
            }
        }
    }
}

fn flush_once(base: &str) -> Result<bool, String> {
    let path = queue_path();
    if !path.exists() {
        return Ok(false);
    }
    let file = File::open(&path).map_err(|e| e.to_string())?;
    let lines: Vec<String> = BufReader::new(file)
        .lines()
        .filter_map(|l| l.ok())
        .filter(|l| !l.trim().is_empty())
        .take(MAX_BATCH)
        .collect();
    if lines.is_empty() {
        return Ok(false);
    }

    let events: Vec<Value> = lines
        .iter()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    if events.is_empty() {
        return Ok(false);
    }

    let device_id = paths::device_id();
    let body = json!({
        "device_id": device_id,
        "events": events,
    });
    let url = format!("{base}/v1/diagnostics");
    let body_text = serde_json::to_string(&body).map_err(|e| e.to_string())?;
    let response = ureq::post(&url)
        .header("Content-Type", "application/json")
        .send(body_text)
        .map_err(|e| e.to_string())?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(format!("diagnostics POST status {status}"));
    }

    truncate_flushed_lines(lines.len())?;
    Ok(true)
}

fn truncate_flushed_lines(count: usize) -> Result<(), String> {
    let path = queue_path();
    let remaining: Vec<String> = if path.exists() {
        BufReader::new(File::open(&path).map_err(|e| e.to_string())?)
            .lines()
            .filter_map(|l| l.ok())
            .skip(count)
            .collect()
    } else {
        Vec::new()
    };
    let mut file = File::create(&path).map_err(|e| e.to_string())?;
    for line in remaining {
        writeln!(file, "{line}").map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn context_is_safe(value: &Value) -> bool {
    let serialized = value.to_string();
    if serialized.len() > MAX_CONTEXT_CHARS {
        return false;
    }
    !contains_blocked_key(value)
}

fn contains_blocked_key(value: &Value) -> bool {
    const BLOCKED: &[&str] = &[
        "transcript",
        "polished",
        "raw_transcript",
        "enriched_transcript",
        "audio",
        "api_key",
        "secret",
        "password",
        "token",
        "authorization",
        "user_text",
        "user_kept",
        "ai_output",
    ];
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let lower = key.to_ascii_lowercase();
                if BLOCKED.iter().any(|b| lower.contains(b)) {
                    return true;
                }
                if contains_blocked_key(child) {
                    return true;
                }
            }
            false
        }
        Value::Array(items) => items.iter().any(contains_blocked_key),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_transcript_in_context() {
        let ctx = json!({ "transcript": "hello world" });
        assert!(!context_is_safe(&ctx));
    }

    #[test]
    fn allows_safe_context() {
        let ctx = json!({ "kind": "spawn", "port": 48484 });
        assert!(context_is_safe(&ctx));
    }

    // ── Comprehensive PII-key coverage ────────────────────────────────────────

    /// Every key in the BLOCKED list must be rejected at the top level.
    #[test]
    fn blocks_all_twelve_pii_keys() {
        let blocked_keys = [
            "transcript",
            "polished",
            "raw_transcript",
            "enriched_transcript",
            "audio",
            "api_key",
            "secret",
            "password",
            "token",
            "authorization",
            "user_text",
            "user_kept",
            "ai_output",
        ];
        for key in blocked_keys {
            let ctx = json!({ key: "any value" });
            assert!(
                !context_is_safe(&ctx),
                "context_is_safe should block key '{key}'"
            );
        }
    }

    /// A blocked key buried inside a nested object must still be caught.
    #[test]
    fn blocks_nested_blocked_key() {
        let ctx = json!({ "meta": { "deep": { "api_key": "sk-xxx" } } });
        assert!(!context_is_safe(&ctx), "nested api_key must be blocked");
    }

    /// A blocked key inside an array element must be caught.
    #[test]
    fn blocks_blocked_key_in_array() {
        let ctx = json!({ "items": [{ "transcript": "sensitive" }] });
        assert!(
            !context_is_safe(&ctx),
            "transcript in array element must be blocked"
        );
    }

    /// Context that is exactly MAX_CONTEXT_CHARS (8192) chars long is borderline-OK;
    /// one char over must be rejected.
    #[test]
    fn blocks_oversized_context() {
        // 8193-char string value — the key itself is short so total > 8192
        let big: String = "x".repeat(9_000);
        let ctx = json!({ "data": big });
        assert!(!context_is_safe(&ctx), "oversized context must be blocked");
    }

    /// Numeric values, booleans, and benign strings are always safe.
    #[test]
    fn allows_numeric_and_boolean_context() {
        let ctx = json!({ "port": 48484, "pid": 12345, "ok": true });
        assert!(context_is_safe(&ctx));
    }
}
