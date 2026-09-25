//! `PUT /v1/recordings/:id/kept` — History keeps what the user kept.
//!
//! Runs the shipped router against a real SQLite file over real HTTP.

use std::{collections::HashMap, sync::Arc};

use said_backend::{
    AppState, router_with_state,
    store::{self, history},
    watchdog::WatchdogState,
};
use serde_json::{Value, json};
use tokio::sync::RwLock;

const SECRET: &str = "kept-text-test-secret";
const PASTED: &str = "Can we close the design review today?";

struct Backend {
    url: String,
    client: reqwest::Client,
    _dir: tempdir::Dir,
}

mod tempdir {
    pub struct Dir(pub std::path::PathBuf);
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

async fn start_backend(recording_id: &str) -> Backend {
    let dir = std::env::temp_dir().join(format!("airnote-kept-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let pool = store::open(&dir.join("said.db"));
    let user_id = store::ensure_default_user(&pool);
    history::insert_recording(
        &pool,
        history::InsertRecording {
            id: recording_id,
            user_id: &user_id,
            transcript: "can we close the design review today",
            polished: PASTED,
            word_count: 7,
            recording_seconds: 2.4,
            model_used: "test",
            confidence: None,
            transcribe_ms: None,
            embed_ms: None,
            polish_ms: None,
            target_app: Some("com.tinyspeck.slackmacgap"),
            source: "voice",
            audio_id: None,
            enriched_transcript: None,
            raw_transcript: None,
            local_corrected_transcript: None,
            polished_output: Some(PASTED),
            trace_json: None,
        },
    )
    .expect("insert recording");

    let state = AppState {
        pool,
        shared_secret: Arc::new(SECRET.to_string()),
        default_user_id: Arc::new(user_id),
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
            .unwrap();
    });
    Backend {
        url,
        client: reqwest::Client::new(),
        _dir: tempdir::Dir(dir),
    }
}

impl Backend {
    async fn put_kept(&self, id: &str, text: &str) -> u16 {
        self.put_kept_learning(id, text).await.0
    }

    /// Status, and the words the route learned from the edit.
    async fn put_kept_learning(&self, id: &str, text: &str) -> (u16, Vec<Value>) {
        let resp = self
            .client
            .put(format!("{}/v1/recordings/{id}/kept", self.url))
            .bearer_auth(SECRET)
            .json(&json!({ "text": text }))
            .send()
            .await
            .unwrap();
        let status = resp.status().as_u16();
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        let learned = body["learned"].as_array().cloned().unwrap_or_default();
        (status, learned)
    }

    async fn dictionary(&self) -> Vec<Value> {
        let body: Value = self
            .client
            .get(format!("{}/v1/dictionary", self.url))
            .bearer_auth(SECRET)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        body["entries"].as_array().cloned().unwrap_or_default()
    }

    async fn history_row(&self, id: &str) -> Value {
        let rows: Vec<Value> = self
            .client
            .get(format!("{}/v1/history", self.url))
            .bearer_auth(SECRET)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        rows.into_iter()
            .find(|row| row["id"] == id)
            .expect("recording in history")
    }
}

#[tokio::test]
async fn history_shows_the_edited_text_and_keeps_airnotes_version() {
    let backend = start_backend("rec-edited").await;
    let kept = "Can we close the design review tomorrow?";

    assert_eq!(backend.put_kept("rec-edited", kept).await, 200);

    let row = backend.history_row("rec-edited").await;
    assert_eq!(row["final_text"], kept);
    assert_eq!(row["polished"], PASTED, "AirNote's version stays available");
    assert_eq!(row["edit_count"], 1);
}

#[tokio::test]
async fn a_second_edit_replaces_the_first() {
    let backend = start_backend("rec-twice").await;
    assert_eq!(
        backend
            .put_kept("rec-twice", "Close the review today?")
            .await,
        200
    );
    assert_eq!(
        backend
            .put_kept("rec-twice", "Close the review Friday?")
            .await,
        200
    );

    let row = backend.history_row("rec-twice").await;
    assert_eq!(row["final_text"], "Close the review Friday?");
    assert_eq!(row["edit_count"], 2);
}

#[tokio::test]
async fn nothing_kept_and_unknown_dictations_are_refused() {
    let backend = start_backend("rec-refused").await;
    assert_eq!(backend.put_kept("rec-refused", "   ").await, 400);
    assert_eq!(backend.put_kept("no-such-recording", "text").await, 404);

    let row = backend.history_row("rec-refused").await;
    assert!(
        row["final_text"].is_null(),
        "a refused write leaves History alone"
    );
}

#[tokio::test]
async fn the_kept_route_needs_the_shared_secret() {
    let backend = start_backend("rec-auth").await;
    let status = backend
        .client
        .put(format!("{}/v1/recordings/rec-auth/kept", backend.url))
        .json(&json!({ "text": "sneaky" }))
        .send()
        .await
        .unwrap()
        .status()
        .as_u16();
    assert_eq!(status, 401);
}

#[tokio::test]
async fn a_corrected_name_joins_the_dictionary() {
    let backend = start_backend("rec-learn").await;
    let (status, learned) = backend
        .put_kept_learning("rec-learn", "Can we close the Figma review today?")
        .await;
    assert_eq!(status, 200);
    assert_eq!(learned.len(), 1, "learned: {learned:?}");
    assert_eq!(learned[0]["heard"], "design");
    assert_eq!(learned[0]["written"], "Figma");

    let entries = backend.dictionary().await;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["source"], "learned");
}

#[tokio::test]
async fn a_rewrite_teaches_nothing() {
    let backend = start_backend("rec-rewrite").await;
    let (status, learned) = backend
        .put_kept_learning("rec-rewrite", "Let's wrap up the Figma review tomorrow")
        .await;
    assert_eq!(status, 200);
    assert!(learned.is_empty(), "learned: {learned:?}");
    assert!(backend.dictionary().await.is_empty());
}
