use said_backend::{AppState, router_with_state, store, watchdog};
use said_core::polish::model::{S1_MINI_FILENAME, S1_MINI_MODEL_KEY, S1_MINI_SIZE_BYTES};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{Mutex, RwLock};

const RAW: &str =
    "so um i need to like send the the report by uh friday no wait make that thursday";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the official s1-mini-q4_k_m.gguf in AirNote's models directory"]
async fn selected_s1_cleans_and_persists_a_local_transcript_without_cloud() {
    let model_path = said_core::paths::data_dir()
        .join("models")
        .join(S1_MINI_FILENAME);
    let metadata = std::fs::metadata(&model_path).unwrap_or_else(|_| {
        panic!(
            "download the official model to {} before running this test",
            model_path.display()
        )
    });
    assert_eq!(metadata.len(), S1_MINI_SIZE_BYTES);

    let db_path = std::env::temp_dir().join(format!(
        "airnote-s1-integration-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    let pool = store::open(&db_path);
    let user_id = store::ensure_default_user(&pool);
    let state = AppState {
        pool: pool.clone(),
        shared_secret: Arc::new("test-secret".into()),
        default_user_id: Arc::new(user_id.clone()),
        prefs_cache: Arc::new(RwLock::new(None)),
        lexicon_cache: Arc::new(RwLock::new(None)),
        live_server_runtime_cache: Arc::new(RwLock::new(HashMap::new())),
        voice_run_hub: Arc::new(Mutex::new(HashMap::new())),
        http_client: reqwest::Client::new(),
        watchdog: Arc::new(watchdog::WatchdogState::new()),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, router_with_state(state))
            .await
            .unwrap();
    });
    let client = reqwest::Client::new();
    let url = |path: &str| format!("http://{address}{path}");

    let patched: Value = client
        .patch(url("/v1/preferences"))
        .bearer_auth("test-secret")
        .json(&json!({ "selected_model": S1_MINI_MODEL_KEY }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(patched["selected_model"], S1_MINI_MODEL_KEY);
    let read_back: Value = client
        .get(url("/v1/preferences"))
        .bearer_auth("test-secret")
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(read_back["selected_model"], S1_MINI_MODEL_KEY);

    // This fake workspace identity unlocks the transcript endpoint. Its closed
    // loopback URL makes any accidental cloud route fail the test.
    store::users::update_enterprise_auth(
        &pool,
        &user_id,
        "test-only-token",
        "enterprise",
        Some("s1-test@example.invalid"),
        Some("http://127.0.0.1:9"),
        Some("S1 integration test"),
    );
    let body = client
        .post(url("/v1/voice/polish-transcript"))
        .bearer_auth("test-secret")
        .json(&json!({ "transcript": RAW, "target_app": "test.notes" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .text()
        .await
        .unwrap();
    let done: Value = body
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|line| serde_json::from_str(line).ok())
        .find(|event: &Value| event.get("model_used").is_some())
        .unwrap_or_else(|| panic!("S1 did not finish successfully:\n{body}"));
    let cleaned = done["polished"].as_str().unwrap();
    assert_ne!(cleaned, RAW);
    assert!(cleaned.ends_with("send the report by Thursday."));
    assert!(!cleaned.contains(" um ") && !cleaned.contains("Friday") && !cleaned.contains("wait"));
    assert_eq!(done["model_used"], "s1_mini:superwhisper/s1-mini");

    let records = store::history::list_recordings(&pool, &user_id, 10, None);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].raw_transcript.as_deref(), Some(RAW));
    assert_eq!(records[0].polished_output.as_deref(), Some(cleaned));
    assert_eq!(records[0].model_used, "s1_mini:superwhisper/s1-mini");

    // Rapid consecutive dictations must queue instead of losing the second
    // take with a busy error. Both completions must still identify local S1.
    let requests = (0..2).map(|_| {
        let request = client
            .post(url("/v1/voice/polish-transcript"))
            .bearer_auth("test-secret")
            .json(&json!({ "transcript": RAW }));
        async move { request.send().await.unwrap().text().await.unwrap() }
    });
    for response in futures::future::join_all(requests).await {
        assert!(response.contains("event: done"), "{response}");
        assert!(
            response.contains("s1_mini:superwhisper/s1-mini"),
            "{response}"
        );
        assert!(!response.contains("event: error"), "{response}");
    }

    server.abort();
    let _ = server.await;
    drop(pool);
    let _ = std::fs::remove_file(db_path);
}
