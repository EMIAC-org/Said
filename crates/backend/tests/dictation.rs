//! What a dictation types: Whisper's transcript as it is with polish off, the
//! model's reply as it is with polish on, and the transcript again when the
//! server cannot polish.
//!
//! Runs the shipped router against a real SQLite file over real HTTP, with a
//! stand-in control plane that records what it was sent.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use said_backend::{
    AppState, router_with_state,
    store::{self, dictionary},
    watchdog::WatchdogState,
};
use serde_json::{Value, json};
use tokio::sync::RwLock;

const SECRET: &str = "dictation-test-secret";
const SPOKEN: &str = "um bhai air note ka build paanch baje tak bhej do";
const MODEL_REPLY: &str = "Bhai, AirNote ka build 5 baje tak bhej do.";

#[derive(Clone, Default)]
struct ControlPlane {
    requests: Arc<Mutex<Vec<Value>>>,
    fail: bool,
}

async fn polish_stream(State(cp): State<ControlPlane>, Json(body): Json<Value>) -> Response {
    cp.requests.lock().unwrap().push(body);
    if cp.fail {
        return (StatusCode::BAD_GATEWAY, "DeepInfra returned 503").into_response();
    }
    let mut sse = String::new();
    for token in ["Bhai, AirNote", " ka build 5", " baje tak bhej do."] {
        sse.push_str(&format!(
            "event: token\ndata: {}\n\n",
            json!({ "token": token })
        ));
    }
    sse.push_str(&format!(
        "event: done\ndata: {}\n\n",
        json!({
            "output": MODEL_REPLY,
            "model_used": "gemma-test",
            "latency_ms": { "prompt": 1, "model": 3, "total": 5 },
        })
    ));
    ([("content-type", "text/event-stream")], sse).into_response()
}

async fn start_control_plane(fail: bool) -> (String, ControlPlane) {
    let cp = ControlPlane {
        fail,
        ..Default::default()
    };
    let app = Router::new()
        .route("/v1/runtime/voice/polish/stream", post(polish_stream))
        .with_state(cp.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, cp)
}

struct Backend {
    url: String,
    pool: store::DbPool,
    user_id: String,
    _dir: TempDir,
}

struct TempDir(std::path::PathBuf);
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn start_backend(control_plane_url: &str, polish_enabled: bool) -> Backend {
    let dir = std::env::temp_dir().join(format!("airnote-dictation-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let pool = store::open(&dir.join("said.db"));
    let user_id = store::ensure_default_user(&pool);
    {
        let conn = pool.get().unwrap();
        conn.execute(
            "UPDATE local_user SET cloud_token = 'token', enterprise_server_url = ?1",
            [control_plane_url],
        )
        .unwrap();
        conn.execute(
            "UPDATE preferences SET polish_enabled = ?1",
            [i64::from(polish_enabled)],
        )
        .unwrap();
    }

    let state = AppState {
        pool: pool.clone(),
        shared_secret: Arc::new(SECRET.to_string()),
        default_user_id: Arc::new(user_id.clone()),
        prefs_cache: Arc::new(RwLock::new(None)),
        live_server_runtime_cache: Arc::new(RwLock::new(HashMap::new())),
        http_client: reqwest::Client::new(),
        watchdog: Arc::new(WatchdogState::new()),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(listener, router_with_state(state))
            .await
            .unwrap()
    });
    Backend {
        url,
        pool,
        user_id,
        _dir: TempDir(dir),
    }
}

impl Backend {
    /// Dictate `transcript` and return the `done` event the desktop types from.
    async fn dictate(&self, transcript: &str) -> Value {
        let body = reqwest::Client::new()
            .post(format!("{}/v1/voice/polish-transcript", self.url))
            .bearer_auth(SECRET)
            .json(&json!({ "transcript": transcript }))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        let mut event = "";
        for line in body.lines() {
            if let Some(name) = line.strip_prefix("event:") {
                event = name.trim();
            } else if let Some(data) = line.strip_prefix("data:") {
                if event == "done" {
                    return serde_json::from_str(data.trim()).unwrap();
                }
            }
        }
        panic!("no done event in: {body}");
    }
}

#[tokio::test]
async fn with_polish_off_the_transcript_is_typed_as_spoken() {
    let (cp_url, cp) = start_control_plane(false).await;
    let backend = start_backend(&cp_url, false).await;

    let done = backend.dictate(SPOKEN).await;

    assert_eq!(done["polished"], SPOKEN);
    assert_eq!(done["model_used"], "polish_disabled");
    assert!(cp.requests.lock().unwrap().is_empty(), "no polish call");
}

#[tokio::test]
async fn with_polish_on_the_model_reply_is_typed_as_returned() {
    let (cp_url, cp) = start_control_plane(false).await;
    let backend = start_backend(&cp_url, true).await;

    let done = backend.dictate(SPOKEN).await;

    assert_eq!(done["polished"], MODEL_REPLY);
    let requests = cp.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0]["transcript"], SPOKEN,
        "the transcript reaches the model untouched"
    );
    assert!(requests[0].get("dictionary").is_none());
}

#[tokio::test]
async fn the_users_words_in_the_transcript_are_sent_with_it() {
    let (cp_url, cp) = start_control_plane(false).await;
    let backend = start_backend(&cp_url, true).await;
    dictionary::add(
        &backend.pool,
        &backend.user_id,
        "AirNote",
        Some("air note"),
        dictionary::SOURCE_LEARNED,
    )
    .unwrap();
    dictionary::add(
        &backend.pool,
        &backend.user_id,
        "Semrush",
        Some("summer rush"),
        dictionary::SOURCE_ADDED,
    )
    .unwrap();

    backend.dictate(SPOKEN).await;

    let requests = cp.requests.lock().unwrap();
    assert_eq!(
        requests[0]["dictionary"],
        json!([{ "heard": "air note", "written": "AirNote" }]),
        "only the word that occurs in the transcript"
    );
}

#[tokio::test]
async fn when_the_server_cannot_polish_the_transcript_is_typed() {
    let (cp_url, _cp) = start_control_plane(true).await;
    let backend = start_backend(&cp_url, true).await;

    let done = backend.dictate(SPOKEN).await;

    assert_eq!(done["polished"], SPOKEN);
    assert_eq!(done["model_used"], "polish_failed");
}
