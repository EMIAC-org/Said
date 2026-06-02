//! Divo agent bridge (desktop side).
//!
//! When the user holds Ctrl and speaks, the recording is transcribed + polished by
//! the normal pipeline, then — instead of being pasted — the plain text is sent
//! here. We POST it to the control-plane Divo proxy (`/v1/divo/chat`) and stream
//! the SSE response back, re-emitting each frame as a `divo-*` Tauri event the HUD
//! renders live. The control-plane attaches the user's Lark token; the desktop only
//! ever holds its own control-plane session token (pushed from the webview).
//!
//! The SSE socket is consumed here in Rust (not the webview), so hiding the HUD —
//! or the WebView being throttled in the background — can never stall the stream.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use futures::StreamExt;
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, Manager};

/// Control-plane base URL + session token, pushed from the webview after login.
#[derive(Clone)]
pub struct Credentials {
    pub server_url: String,
    pub token: String,
}

/// Managed state for the Divo channel.
pub struct DivoState {
    creds: Mutex<Option<Credentials>>,
    /// Active thread id (from the `meta`/`done` frame), reused for follow-ups.
    thread_id: Mutex<Option<String>>,
    /// Monotonic turn counter — a stream whose turn is stale stops emitting, so a
    /// fresh Ctrl press cleanly supersedes an in-flight run.
    turn: AtomicU64,
}

impl DivoState {
    pub fn new() -> Self {
        Self {
            creds: Mutex::new(None),
            thread_id: Mutex::new(None),
            turn: AtomicU64::new(0),
        }
    }

    pub fn set_credentials(&self, server_url: String, token: String) {
        if let Ok(mut g) = self.creds.lock() {
            *g = if server_url.trim().is_empty() || token.trim().is_empty() {
                None
            } else {
                Some(Credentials { server_url, token })
            };
        }
    }

    pub fn credentials(&self) -> Option<Credentials> {
        self.creds.lock().ok().and_then(|g| g.clone())
    }

    pub fn current_thread(&self) -> Option<String> {
        self.thread_id.lock().ok().and_then(|g| g.clone())
    }

    pub fn set_thread(&self, id: String) {
        if let Ok(mut g) = self.thread_id.lock() {
            *g = Some(id);
        }
    }

    pub fn clear_thread(&self) {
        if let Ok(mut g) = self.thread_id.lock() {
            *g = None;
        }
    }

    fn next_turn(&self) -> u64 {
        self.turn.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn current_turn(&self) -> u64 {
        self.turn.load(Ordering::SeqCst)
    }
}

impl Default for DivoState {
    fn default() -> Self {
        Self::new()
    }
}

fn emit_error(app: &AppHandle, message: &str) {
    let _ = app.emit("divo-error", json!({ "message": message }));
}

/// Resolve where to send a Divo request.
///
/// Dev-direct (env `AIRNOTE_DIVO_DIRECT` = the Divo base URL): the desktop talks
/// straight to Divo's `/api/airnote/*` with a Lark token from
/// `AIRNOTE_DIVO_DIRECT_TOKEN`, bypassing the control-plane. This is **local
/// testing only** — production leaves the env unset and goes through the
/// control-plane proxy with the pushed session token.
///
/// Returns `(base_url, bearer_token, direct)`.
fn resolve_target(state: &DivoState) -> Option<(String, String, bool)> {
    if let Ok(base) = std::env::var("AIRNOTE_DIVO_DIRECT") {
        let base = base.trim().to_string();
        if !base.is_empty() {
            let token = std::env::var("AIRNOTE_DIVO_DIRECT_TOKEN").unwrap_or_default();
            return Some((base, token, true));
        }
    }
    let c = state.credentials()?;
    Some((c.server_url, c.token, false))
}

fn chat_url(base: &str, direct: bool) -> String {
    let base = base.trim_end_matches('/');
    if direct {
        format!("{base}/api/airnote/chat")
    } else {
        format!("{base}/v1/divo/chat")
    }
}

fn thread_url(base: &str, direct: bool, thread_id: &str) -> String {
    let base = base.trim_end_matches('/');
    if direct {
        format!("{base}/api/airnote/threads/{thread_id}?page=1&pageSize=50")
    } else {
        format!("{base}/v1/divo/threads/{thread_id}?page=1&pageSize=50")
    }
}

fn list_url(base: &str, direct: bool) -> String {
    let base = base.trim_end_matches('/');
    if direct {
        format!("{base}/api/airnote/threads?page=1&pageSize=30")
    } else {
        format!("{base}/v1/divo/threads?page=1&pageSize=30")
    }
}

/// Kick off a Divo turn for `message`. `thread_id = Some(..)` continues an existing
/// thread (a spoken follow-up); `None` starts a fresh task. Returns immediately —
/// the SSE is consumed on a background task that emits `divo-*` events.
pub fn send_instruction(app: AppHandle, message: String, thread_id: Option<String>) {
    let state = app.state::<DivoState>();
    let (base, token, direct) = match resolve_target(&state) {
        Some(t) => t,
        None => {
            emit_error(&app, "Sign in with Lark to use Divo.");
            return;
        }
    };
    if thread_id.is_none() {
        state.clear_thread();
    }
    let turn = state.next_turn();
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        stream_chat(app2, base, token, direct, message, thread_id, turn).await;
    });
}

/// Send a reviewed/edited instruction to Divo — invoked by the staging HUD's
/// Send button. Transcription/polish already happened; this is the explicit
/// commit. `thread_id = Some(..)` routes the turn into that chat (continue an
/// existing thread or a chat picked in the router); `None` starts a new chat.
#[tauri::command]
pub fn divo_send(app: AppHandle, message: String, thread_id: Option<String>) {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return;
    }
    let thread = thread_id
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());
    tracing::info!(
        "[divo] sending reviewed instruction ({} chars, thread={:?})",
        trimmed.len(),
        thread.as_deref()
    );
    send_instruction(app, trimmed.to_string(), thread);
}

async fn stream_chat(
    app: AppHandle,
    base: String,
    token: String,
    direct: bool,
    message: String,
    thread_id: Option<String>,
    turn: u64,
) {
    let is_followup = thread_id.is_some();
    let _ = app.emit("divo-started", json!({ "followup": is_followup }));

    let url = chat_url(&base, direct);
    let request_id = uuid::Uuid::new_v4().to_string();
    let mut body = json!({ "requestId": request_id, "message": message, "mode": "high" });
    if let Some(tid) = &thread_id {
        body["threadId"] = json!(tid);
    }

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "text/event-stream")
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await;

    let resp = match resp {
        Ok(r) => r,
        Err(e) => {
            emit_error(&app, &format!("Couldn't reach Divo: {e}"));
            return;
        }
    };

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        let msg = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
            .unwrap_or_else(|| {
                if text.is_empty() {
                    format!("Divo error ({})", status.as_u16())
                } else {
                    text
                }
            });
        emit_error(&app, &msg);
        return;
    }

    consume_divo_sse(app, resp.bytes_stream(), turn).await;
}

async fn consume_divo_sse<S>(app: AppHandle, mut stream: S, turn: u64)
where
    S: StreamExt<Item = Result<bytes::Bytes, reqwest::Error>> + Unpin,
{
    let mut buf = String::new();
    let mut event_name = String::new();
    let mut saw_terminal = false;
    let mut pending_approval: Option<String> = None;

    while let Some(chunk) = stream.next().await {
        // A newer turn started (fresh Ctrl press) — abandon this stream silently.
        if app.state::<DivoState>().current_turn() != turn {
            return;
        }
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                emit_error(&app, &format!("Divo stream interrupted: {e}"));
                return;
            }
        };
        buf.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(nl) = buf.find('\n') {
            let line = buf[..nl].trim().to_string();
            buf = buf[nl + 1..].to_string();

            if line.is_empty() {
                event_name.clear();
                continue;
            }
            if let Some(name) = line.strip_prefix("event:") {
                event_name = name.trim().to_string();
                continue;
            }
            // Anything that isn't a `data:` line (including `: ping` heartbeats) is ignored.
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data.is_empty() {
                continue;
            }
            let Ok(val) = serde_json::from_str::<Value>(data) else {
                continue;
            };
            dispatch(
                &app,
                &event_name,
                &val,
                &mut saw_terminal,
                &mut pending_approval,
            );
            if saw_terminal {
                return;
            }
        }
    }

    // Stream closed without a terminal `done`/`error`.
    if !saw_terminal {
        if let Some(label) = pending_approval {
            let _ = app.emit("divo-pending", json!({ "message": label }));
        } else {
            emit_error(&app, "Divo stopped responding.");
        }
    }
}

/// The HUD events + side effects a single Divo SSE frame maps to. Kept separate
/// from `AppHandle` so [`classify_frame`] is pure and unit-testable.
#[derive(Default)]
struct FrameOut {
    emits: Vec<(&'static str, Value)>,
    set_thread: Option<String>,
    pending: Option<String>,
    terminal: bool,
}

/// Pure mapping of one Divo SSE frame (`event:` name + parsed `data:` JSON) to the
/// `divo-*` events the HUD should receive. No I/O, no app state — see tests below.
fn classify_frame(event_name: &str, val: &Value) -> FrameOut {
    let mut out = FrameOut::default();
    match event_name {
        "meta" => {
            if let Some(tid) = val.get("threadId").and_then(Value::as_str) {
                out.set_thread = Some(tid.to_string());
                out.emits.push(("divo-meta", json!({ "threadId": tid })));
            }
        }
        "status" => {
            out.emits.push(("divo-status", val.clone()));
            if val.get("phase").and_then(Value::as_str) == Some("awaiting_approval") {
                out.pending = Some(
                    val.get("liveLabel")
                        .and_then(Value::as_str)
                        .unwrap_or("Pending approval in Lark")
                        .to_string(),
                );
            }
        }
        "thinking" => {
            if let Some(text) = val.get("text").and_then(Value::as_str) {
                out.emits.push(("divo-thinking", json!({ "text": text })));
            }
        }
        "tool.start" => {
            out.emits.push((
                "divo-tool",
                json!({
                    "phase": "start",
                    "name": val.get("name").and_then(Value::as_str).unwrap_or(""),
                    "family": val.get("family").and_then(Value::as_str),
                    "verb": val.get("verb").and_then(Value::as_str),
                    "callId": val.get("callId").and_then(Value::as_str),
                }),
            ));
        }
        "tool.end" => {
            out.emits.push((
                "divo-tool",
                json!({
                    "phase": "end",
                    "name": val.get("name").and_then(Value::as_str).unwrap_or(""),
                    "ok": val.get("ok").and_then(Value::as_bool).unwrap_or(true),
                    "past": val.get("past").and_then(Value::as_str),
                    "callId": val.get("callId").and_then(Value::as_str),
                }),
            ));
        }
        "done" => {
            let content = val
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let tid = val
                .get("message")
                .and_then(|m| m.get("threadId"))
                .and_then(Value::as_str);
            if let Some(t) = tid {
                out.set_thread = Some(t.to_string());
            }
            out.emits
                .push(("divo-done", json!({ "content": content, "threadId": tid })));
            out.terminal = true;
        }
        "error" => {
            let msg = val
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Divo failed")
                .to_string();
            out.emits.push(("divo-error", json!({ "message": msg })));
            out.terminal = true;
        }
        // `text` deltas and any unknown frames are ignored — we render from the
        // rolling status/tool stream and the final `done` answer.
        _ => {}
    }
    out
}

fn dispatch(
    app: &AppHandle,
    event_name: &str,
    val: &Value,
    saw_terminal: &mut bool,
    pending_approval: &mut Option<String>,
) {
    let out = classify_frame(event_name, val);
    if let Some(tid) = out.set_thread {
        app.state::<DivoState>().set_thread(tid);
    }
    for (event, payload) in out.emits {
        let _ = app.emit(event, payload);
    }
    if let Some(label) = out.pending {
        *pending_approval = Some(label);
    }
    if out.terminal {
        *saw_terminal = true;
    }
}

// ── Tauri commands ───────────────────────────────────────────────────────────

/// Push the control-plane base URL + session token from the webview. Enables (or,
/// when blank, disables) the Ctrl hold-to-talk Divo hotkey.
#[tauri::command]
pub fn divo_set_credentials(server_url: String, token: String, state: tauri::State<'_, DivoState>) {
    let enabled = !server_url.trim().is_empty() && !token.trim().is_empty();
    state.set_credentials(server_url, token);
    said_hotkey::set_divo_hotkey_enabled(enabled);
    tracing::info!("[divo] credentials updated — hotkey enabled={enabled}");
}

/// Recover the latest assistant answer for a thread (used after the SSE dropped, or
/// after a Lark approval lands). Returns the markdown content, if any.
#[tauri::command]
pub async fn divo_fetch_thread(
    thread_id: String,
    state: tauri::State<'_, DivoState>,
) -> Result<Option<String>, String> {
    let (base, token, direct) = resolve_target(&state).ok_or("not connected to Divo")?;
    let url = thread_url(&base, direct, &thread_id);
    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| format!("divo unreachable: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("divo error ({})", resp.status().as_u16()));
    }
    let val: Value = resp
        .json()
        .await
        .map_err(|e| format!("bad response: {e}"))?;
    // Latest assistant message's content.
    let content = val
        .get("data")
        .and_then(|d| d.get("messages"))
        .and_then(|m| m.as_array())
        .and_then(|msgs| {
            msgs.iter()
                .rev()
                .find(|m| m.get("role").and_then(Value::as_str) == Some("assistant"))
                .and_then(|m| m.get("content").and_then(Value::as_str))
                .map(str::to_string)
        });
    Ok(content)
}

/// List the user's AirNote Divo chats — backs the in-app history list and the
/// HUD chat router. Returns the proxy JSON verbatim (`{ data: { threads, … } }`).
#[tauri::command]
pub async fn divo_list_threads(state: tauri::State<'_, DivoState>) -> Result<Value, String> {
    let (base, token, direct) = resolve_target(&state).ok_or("not connected to Divo")?;
    let url = list_url(&base, direct);
    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| format!("divo unreachable: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("divo error ({})", resp.status().as_u16()));
    }
    resp.json().await.map_err(|e| format!("bad response: {e}"))
}

/// Fetch a full thread (all messages) for the in-app conversation pane. Returns the
/// proxy JSON verbatim (`{ data: { id, title, messages, … } }`).
#[tauri::command]
pub async fn divo_thread_messages(
    thread_id: String,
    state: tauri::State<'_, DivoState>,
) -> Result<Value, String> {
    let (base, token, direct) = resolve_target(&state).ok_or("not connected to Divo")?;
    let url = thread_url(&base, direct, &thread_id);
    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| format!("divo unreachable: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("divo error ({})", resp.status().as_u16()));
    }
    resp.json().await.map_err(|e| format!("bad response: {e}"))
}

/// Mark a thread as the active one so a plain Ctrl hold continues it — or clear it
/// (`None`) so the next Ctrl press starts fresh. Called when the user opens a chat
/// in the in-app Divo section.
#[tauri::command]
pub fn divo_set_active_thread(thread_id: Option<String>, state: tauri::State<'_, DivoState>) {
    match thread_id
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
    {
        Some(id) => state.set_thread(id),
        None => state.clear_thread(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Value {
        serde_json::from_str(s).unwrap()
    }

    // Frames below are taken verbatim from the Divo build spec §9 example.

    #[test]
    fn meta_stores_thread_and_emits() {
        let out = classify_frame("meta", &v(r#"{"threadId":"a1b2c3","requestId":"1111"}"#));
        assert_eq!(out.set_thread.as_deref(), Some("a1b2c3"));
        assert_eq!(out.emits.len(), 1);
        assert_eq!(out.emits[0].0, "divo-meta");
        assert_eq!(out.emits[0].1["threadId"], "a1b2c3");
        assert!(!out.terminal);
    }

    #[test]
    fn status_passes_through_live_label_and_progress() {
        let out = classify_frame("status", &v(r#"{"liveLabel":"Thinking…","progressPct":8}"#));
        assert_eq!(out.emits[0].0, "divo-status");
        assert_eq!(out.emits[0].1["liveLabel"], "Thinking…");
        assert_eq!(out.emits[0].1["progressPct"], 8);
        assert!(out.pending.is_none());
    }

    #[test]
    fn awaiting_approval_marks_pending() {
        let out = classify_frame(
            "status",
            &v(r#"{"phase":"awaiting_approval","liveLabel":"Sent to Asha for approval in Lark"}"#),
        );
        assert_eq!(
            out.pending.as_deref(),
            Some("Sent to Asha for approval in Lark")
        );
        assert!(!out.terminal);
    }

    #[test]
    fn tool_start_and_end_map_to_divo_tool() {
        let start = classify_frame(
            "tool.start",
            &v(
                r#"{"callId":"t1","name":"zohoBooks","family":"zoho","verb":"Searching overdue invoices","args":{}}"#,
            ),
        );
        assert_eq!(start.emits[0].0, "divo-tool");
        assert_eq!(start.emits[0].1["phase"], "start");
        assert_eq!(start.emits[0].1["name"], "zohoBooks");
        assert_eq!(start.emits[0].1["verb"], "Searching overdue invoices");

        let end = classify_frame(
            "tool.end",
            &v(
                r#"{"callId":"t1","name":"zohoBooks","ok":true,"durationMs":1430,"past":"Searched overdue invoices"}"#,
            ),
        );
        assert_eq!(end.emits[0].1["phase"], "end");
        assert_eq!(end.emits[0].1["ok"], true);
        assert_eq!(end.emits[0].1["past"], "Searched overdue invoices");
    }

    #[test]
    fn done_is_terminal_and_carries_markdown() {
        // Built via json! (not a raw string) because the markdown content begins
        // with "###, whose "# would close an r#"…"# literal early.
        let frame = json!({
            "message": {
                "id": "m9",
                "threadId": "a1b2c3",
                "role": "assistant",
                "content": "### Overdue invoices\n\n| Client | Amount |\n|---|---|\n| Acme | ₹1,20,000 |",
                "createdAt": "2026-06-01T00:00:00Z"
            },
            "format": "markdown"
        });
        let out = classify_frame("done", &frame);
        assert!(out.terminal);
        assert_eq!(out.set_thread.as_deref(), Some("a1b2c3"));
        assert_eq!(out.emits[0].0, "divo-done");
        assert!(
            out.emits[0].1["content"]
                .as_str()
                .unwrap()
                .contains("Overdue invoices")
        );
        assert_eq!(out.emits[0].1["threadId"], "a1b2c3");
    }

    #[test]
    fn error_is_terminal() {
        let out = classify_frame("error", &v(r#"{"message":"something broke"}"#));
        assert!(out.terminal);
        assert_eq!(out.emits[0].0, "divo-error");
        assert_eq!(out.emits[0].1["message"], "something broke");
    }

    #[test]
    fn text_and_unknown_frames_are_ignored() {
        assert!(
            classify_frame("text", &v(r#"{"delta":"partial"}"#))
                .emits
                .is_empty()
        );
        assert!(
            classify_frame("whatever", &v(r#"{"x":1}"#))
                .emits
                .is_empty()
        );
    }

    #[test]
    fn urls_pick_direct_vs_proxy_paths() {
        assert_eq!(
            chat_url("http://localhost:8000", true),
            "http://localhost:8000/api/airnote/chat"
        );
        assert_eq!(
            chat_url("https://cp.example/", false),
            "https://cp.example/v1/divo/chat"
        );
        assert_eq!(
            thread_url("http://localhost:8000", true, "abc"),
            "http://localhost:8000/api/airnote/threads/abc?page=1&pageSize=50"
        );
        assert_eq!(
            thread_url("https://cp.example", false, "abc"),
            "https://cp.example/v1/divo/threads/abc?page=1&pageSize=50"
        );
        assert_eq!(
            list_url("http://localhost:8000", true),
            "http://localhost:8000/api/airnote/threads?page=1&pageSize=30"
        );
        assert_eq!(
            list_url("https://cp.example/", false),
            "https://cp.example/v1/divo/threads?page=1&pageSize=30"
        );
    }
}
