//! Integration tests for AirNote Enterprise meeting system.
//!
//! Requires a running Postgres instance. Set DATABASE_URL env var or .env file.
//! Each test uses unique UUIDs in emails/slugs for isolation.
//!
//! Run: cd crates/control-plane && cargo test --test meeting_scenarios

use std::sync::Arc;
use std::time::Instant;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use uuid::Uuid;

use said_control_plane::{AppState, LarkConfig, build_router, meeting_hub, routes, store};

// ═══════════════════════════════════════════════════════════════════════════════
// Test harness
// ═══════════════════════════════════════════════════════════════════════════════

struct TestServer {
    addr: String,
    db: sqlx::PgPool,
    client: reqwest::Client,
}

impl TestServer {
    async fn start() -> Self {
        let _ = dotenvy::dotenv();
        let db_url =
            std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for integration tests");

        let db = store::connect(&db_url).await.expect("connect to Postgres");
        let hub = meeting_hub::MeetingHub::new(db.clone());

        let state = AppState {
            db: db.clone(),
            started_at: Arc::new(Instant::now()),
            lark: LarkConfig {
                app_id: String::new(),
                app_secret: String::new(),
                redirect_uri: String::new(),
                jwt_secret: "test-secret".into(),
            },
            hub,
            notifications: said_control_plane::notification_hub::NotificationHub::new(),
            deepgram_api_key: String::new(),
            stt_provider: "deepgram".to_string(),
            groq_api_key: String::new(),
            diagnostics_rate_limit: routes::diagnostics::DiagnosticsRateLimiter::default(),
            divo_base_url: String::new(),
            runtime_credentials_key: "test-runtime-credentials-key".into(),
            runtime_cipher: None,
            deepseek_api_key: String::new(),
            deepseek_base_url: String::new(),
            deepseek_message_polish_model: String::new(),
        };

        let app = build_router(state);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());

        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        Self {
            addr,
            db,
            client: reqwest::Client::new(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.addr, path)
    }

    fn ws_url(&self, path: &str) -> String {
        self.url(path).replace("http://", "ws://")
    }

    /// Create an account and return (account_id, session_token).
    async fn create_account(&self, suffix: &str) -> (Uuid, Uuid) {
        let email = format!("test-{}@scenario.test", suffix);
        let res = self
            .client
            .post(self.url("/v1/auth/signup"))
            .json(&json!({"email": email, "password": "testpass123"}))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200, "signup failed for {email}");
        let body: Value = res.json().await.unwrap();
        let account_id = Uuid::parse_str(body["account"]["id"].as_str().unwrap()).unwrap();
        let token = Uuid::parse_str(body["token"].as_str().unwrap()).unwrap();
        (account_id, token)
    }

    /// Create an org and add the creator as a member with the given role.
    async fn create_org(
        &self,
        token: Uuid,
        account_id: Uuid,
        slug_suffix: &str,
        role: &str,
    ) -> Uuid {
        let slug = format!("test-org-{slug_suffix}");
        let org_id: Uuid = sqlx::query_scalar(
            "INSERT INTO orgs (name, slug) VALUES ($1, $2)
             ON CONFLICT (slug) DO UPDATE SET name = $1
             RETURNING id",
        )
        .bind(&slug)
        .bind(&slug)
        .fetch_one(&self.db)
        .await
        .unwrap();

        // Upsert member with the specified role
        sqlx::query(
            "INSERT INTO org_members (org_id, account_id, role)
             VALUES ($1, $2, $3)
             ON CONFLICT (org_id, account_id) DO UPDATE SET role = $3",
        )
        .bind(org_id)
        .bind(account_id)
        .bind(role)
        .execute(&self.db)
        .await
        .unwrap();

        let _ = token; // token used for auth context in HTTP calls
        org_id
    }

    /// Create a meeting via the API. Returns meeting_id.
    /// If participant_ids is empty, a dummy peer account is auto-created to
    /// satisfy the "at least one invitee" validation.
    async fn create_meeting(&self, token: Uuid, title: &str, participant_ids: &[Uuid]) -> Uuid {
        let ids: Vec<Uuid> = if participant_ids.is_empty() {
            let (peer_id, _) = self.create_account(&Uuid::new_v4().to_string()).await;
            vec![peer_id]
        } else {
            participant_ids.to_vec()
        };
        let res = self
            .client
            .post(self.url("/v1/meetings"))
            .header("Authorization", format!("Bearer {token}"))
            .json(&json!({
                "title": title,
                "participant_ids": ids,
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(
            res.status(),
            201,
            "create meeting failed: {:?}",
            res.text().await
        );
        let body: Value = res.json().await.unwrap();
        Uuid::parse_str(body["meeting"]["id"].as_str().unwrap()).unwrap()
    }

    /// Start a meeting (set status to 'live').
    async fn start_meeting(&self, token: Uuid, meeting_id: Uuid) {
        let res = self
            .client
            .post(self.url(&format!("/v1/meetings/{meeting_id}/start")))
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200, "start meeting failed");
    }

    /// Connect a WebSocket client. Returns (sink, stream).
    async fn connect_ws(
        &self,
        meeting_id: Uuid,
        token: Uuid,
    ) -> (
        futures_util::stream::SplitSink<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
            Message,
        >,
        futures_util::stream::SplitStream<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
        >,
    ) {
        let url = self.ws_url(&format!("/v1/meetings/{meeting_id}/ws?token={token}"));
        let (ws, _response) = connect_async(&url).await.expect("ws connect failed");
        ws.split()
    }

    /// Read the next text message from a WS stream, parsed as JSON.
    async fn next_ws_json(
        stream: &mut futures_util::stream::SplitStream<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
        >,
    ) -> Value {
        let timeout = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                match stream.next().await {
                    Some(Ok(Message::Text(t))) => {
                        return serde_json::from_str::<Value>(&t).expect("parse ws json");
                    }
                    Some(Ok(_)) => continue,
                    Some(Err(e)) => panic!("ws read error: {e}"),
                    None => panic!("ws stream ended"),
                }
            }
        });
        timeout.await.expect("ws read timed out after 5s")
    }

    /// Drain all messages available within a timeout, return them.
    async fn drain_ws(
        stream: &mut futures_util::stream::SplitStream<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
        >,
        duration: std::time::Duration,
    ) -> Vec<Value> {
        let mut msgs = Vec::new();
        let _ = tokio::time::timeout(duration, async {
            loop {
                match stream.next().await {
                    Some(Ok(Message::Text(t))) => {
                        if let Ok(v) = serde_json::from_str::<Value>(&t) {
                            msgs.push(v);
                        }
                    }
                    Some(Ok(_)) => continue,
                    _ => break,
                }
            }
        })
        .await;
        msgs
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Scenario 1: Health check
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn s01_health_check() {
    let srv = TestServer::start().await;
    let res = srv.client.get(srv.url("/v1/health")).send().await.unwrap();
    assert_eq!(res.status(), 200);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Scenario 2: Signup + login + auth
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn s02_signup_login_auth() {
    let srv = TestServer::start().await;
    let tag = Uuid::new_v4();

    // Signup
    let (account_id, token) = srv.create_account(&tag.to_string()).await;

    // Auth me
    let res = srv
        .client
        .get(srv.url("/v1/auth/me"))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert_eq!(
        body["account"]["id"].as_str().unwrap(),
        account_id.to_string()
    );
}

#[tokio::test]
async fn s02b_first_user_org_onboarding_crud() {
    let srv = TestServer::start().await;
    let tag = Uuid::new_v4();
    let slug = format!("onboard-{tag}");

    let (_account_id, token) = srv.create_account(&format!("onboard-{tag}")).await;

    let missing = srv
        .client
        .get(srv.url("/v1/orgs/me"))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(missing.status(), 404);

    let created = srv
        .client
        .post(srv.url("/v1/orgs"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&json!({
            "name": "Onboarding Test",
            "slug": slug,
            "meeting_creator_roles": ["COMPANY_ADMIN", "MANAGER"],
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 201);
    let created_body: Value = created.json().await.unwrap();
    assert_eq!(created_body["org"]["role"], "COMPANY_ADMIN");

    let me = srv
        .client
        .get(srv.url("/v1/orgs/me"))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(me.status(), 200);
    let me_body: Value = me.json().await.unwrap();
    assert_eq!(me_body["org"]["slug"], created_body["org"]["slug"]);

    let meeting_id = srv
        .create_meeting(token, "Onboarding CRUD Meeting", &[])
        .await;

    let list = srv
        .client
        .get(srv.url("/v1/meetings"))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(list.status(), 200);
    let list_body: Value = list.json().await.unwrap();
    assert!(
        list_body["meetings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["id"] == meeting_id.to_string())
    );

    srv.start_meeting(token, meeting_id).await;

    let ended = srv
        .client
        .post(srv.url(&format!("/v1/meetings/{meeting_id}/end")))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(ended.status(), 200);

    let detail = srv
        .client
        .get(srv.url(&format!("/v1/meetings/{meeting_id}")))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(detail.status(), 200);
    let detail_body: Value = detail.json().await.unwrap();
    assert_eq!(detail_body["meeting"]["status"], "ended");
}

#[tokio::test]
async fn s02c_admin_routes_are_normalized_and_safe() {
    let srv = TestServer::start().await;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    let admin = client.get(srv.url("/admin")).send().await.unwrap();
    assert_eq!(admin.status(), 307);
    assert_eq!(admin.headers().get("location").unwrap(), "/admin/");

    let res = client.get(srv.url("/adminfirst")).send().await.unwrap();
    assert_eq!(res.status(), 404);
    let body = res.text().await.unwrap();
    assert!(body.contains("Page not found"));

    let asset = client
        .get(srv.url("/admin/assets/missing.js"))
        .send()
        .await
        .unwrap();
    assert_eq!(asset.status(), 404);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Scenario 3: MEMBER role blocked from creating meeting
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn s03_member_blocked_from_creating_meeting() {
    let srv = TestServer::start().await;
    let tag = Uuid::new_v4();

    let (account_id, token) = srv.create_account(&format!("member-{tag}")).await;
    srv.create_org(token, account_id, &tag.to_string(), "MEMBER")
        .await;

    let res = srv
        .client
        .post(srv.url("/v1/meetings"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&json!({"title": "test meeting", "participant_ids": []}))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 403);
    let body: Value = res.json().await.unwrap();
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("role does not permit")
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Scenario 3b: Empty participant list rejected
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn s03b_empty_participants_rejected() {
    let srv = TestServer::start().await;
    let tag = Uuid::new_v4();

    let (account_id, token) = srv.create_account(&format!("empty-{tag}")).await;
    srv.create_org(token, account_id, &tag.to_string(), "COMPANY_ADMIN")
        .await;

    let res = srv
        .client
        .post(srv.url("/v1/meetings"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&json!({"title": "Solo meeting", "participant_ids": []}))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 422);
    let body: Value = res.json().await.unwrap();
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("at least one participant")
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Scenario 4: COMPANY_ADMIN can create meeting
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn s04_admin_can_create_meeting() {
    let srv = TestServer::start().await;
    let tag = Uuid::new_v4();

    let (account_id, token) = srv.create_account(&format!("admin-{tag}")).await;
    srv.create_org(token, account_id, &tag.to_string(), "COMPANY_ADMIN")
        .await;
    let (peer_id, _) = srv.create_account(&format!("peer-admin-{tag}")).await;

    let res = srv
        .client
        .post(srv.url("/v1/meetings"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&json!({"title": "Admin meeting", "participant_ids": [peer_id]}))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 201);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Scenario 5: MANAGER can create meeting
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn s05_manager_can_create_meeting() {
    let srv = TestServer::start().await;
    let tag = Uuid::new_v4();

    let (account_id, token) = srv.create_account(&format!("mgr-{tag}")).await;
    srv.create_org(token, account_id, &tag.to_string(), "MANAGER")
        .await;
    let (peer_id, _) = srv.create_account(&format!("peer-mgr-{tag}")).await;

    let res = srv
        .client
        .post(srv.url("/v1/meetings"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&json!({"title": "Manager meeting", "participant_ids": [peer_id]}))
        .send()
        .await
        .unwrap();

    assert_eq!(res.status(), 201);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Scenario 6: Meeting list filtered by status
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn s06_meeting_list_status_filter() {
    let srv = TestServer::start().await;
    let tag = Uuid::new_v4();

    let (account_id, token) = srv.create_account(&format!("list-{tag}")).await;
    srv.create_org(token, account_id, &tag.to_string(), "COMPANY_ADMIN")
        .await;

    // Create two meetings
    let m1 = srv.create_meeting(token, "Scheduled one", &[]).await;
    let m2 = srv.create_meeting(token, "Will be live", &[]).await;

    // Start m2 → becomes live
    srv.start_meeting(token, m2).await;

    // List scheduled only
    let res = srv
        .client
        .get(srv.url("/v1/meetings?status=scheduled"))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    let body: Value = res.json().await.unwrap();
    let meetings = body["meetings"].as_array().unwrap();
    assert!(meetings.iter().all(|m| m["status"] == "scheduled"));
    assert!(meetings.iter().any(|m| m["id"] == m1.to_string()));

    // List live only
    let res = srv
        .client
        .get(srv.url("/v1/meetings?status=live"))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    let body: Value = res.json().await.unwrap();
    let meetings = body["meetings"].as_array().unwrap();
    assert!(meetings.iter().all(|m| m["status"] == "live"));
    assert!(meetings.iter().any(|m| m["id"] == m2.to_string()));
}

// ═══════════════════════════════════════════════════════════════════════════════
// Scenario 7: Meeting end marks all participants as left
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn s07_meeting_end_marks_all_left() {
    let srv = TestServer::start().await;
    let tag = Uuid::new_v4();

    let (admin_id, admin_token) = srv.create_account(&format!("end-admin-{tag}")).await;
    let (user_id, _) = srv.create_account(&format!("end-user-{tag}")).await;
    let org_id = srv
        .create_org(
            admin_token,
            admin_id,
            &format!("end-{tag}"),
            "COMPANY_ADMIN",
        )
        .await;

    // Add user to org
    sqlx::query("INSERT INTO org_members (org_id, account_id, role) VALUES ($1, $2, 'MEMBER')")
        .bind(org_id)
        .bind(user_id)
        .execute(&srv.db)
        .await
        .unwrap();

    let meeting_id = srv
        .create_meeting(admin_token, "End test", &[user_id])
        .await;
    srv.start_meeting(admin_token, meeting_id).await;

    // End it
    let res = srv
        .client
        .post(srv.url(&format!("/v1/meetings/{meeting_id}/end")))
        .header("Authorization", format!("Bearer {admin_token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert!(body["meeting"]["participants_ended"].as_u64().unwrap() >= 2);

    // Verify all participants are 'left'
    let statuses: Vec<(String,)> =
        sqlx::query_as("SELECT status FROM meeting_participants WHERE meeting_id = $1")
            .bind(meeting_id)
            .fetch_all(&srv.db)
            .await
            .unwrap();
    assert!(statuses.iter().all(|(s,)| s == "left"));
}

// ═══════════════════════════════════════════════════════════════════════════════
// Scenario 8: Non-participant rejected from WebSocket
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn s08_non_participant_rejected_from_ws() {
    let srv = TestServer::start().await;
    let tag = Uuid::new_v4();

    let (admin_id, admin_token) = srv.create_account(&format!("ws-admin-{tag}")).await;
    let (outsider_id, outsider_token) = srv.create_account(&format!("ws-outsider-{tag}")).await;

    let org_id = srv
        .create_org(admin_token, admin_id, &format!("ws-{tag}"), "COMPANY_ADMIN")
        .await;
    // outsider is in org but NOT a participant of this specific meeting
    sqlx::query("INSERT INTO org_members (org_id, account_id, role) VALUES ($1, $2, 'MEMBER')")
        .bind(org_id)
        .bind(outsider_id)
        .execute(&srv.db)
        .await
        .unwrap();

    let meeting_id = srv
        .create_meeting(admin_token, "Private meeting", &[])
        .await;
    srv.start_meeting(admin_token, meeting_id).await;

    // Try to connect as outsider — should fail
    let url = srv.ws_url(&format!(
        "/v1/meetings/{meeting_id}/ws?token={outsider_token}"
    ));
    let result = connect_async(&url).await;
    assert!(result.is_err(), "non-participant should be rejected");
}

// ═══════════════════════════════════════════════════════════════════════════════
// Scenario 9: WebSocket connect + welcome + catchup
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn s09_ws_connect_welcome_catchup() {
    let srv = TestServer::start().await;
    let tag = Uuid::new_v4();

    let (admin_id, admin_token) = srv.create_account(&format!("ws-welcome-{tag}")).await;
    srv.create_org(
        admin_token,
        admin_id,
        &format!("wswelc-{tag}"),
        "COMPANY_ADMIN",
    )
    .await;

    let meeting_id = srv.create_meeting(admin_token, "Welcome test", &[]).await;
    srv.start_meeting(admin_token, meeting_id).await;

    let (mut _sink, mut stream) = srv.connect_ws(meeting_id, admin_token).await;

    // First message: connected welcome
    let msg = TestServer::next_ws_json(&mut stream).await;
    assert_eq!(msg["type"], "connected");
    assert_eq!(msg["meeting_id"], meeting_id.to_string());

    // Second message: catchup packet
    let msg = TestServer::next_ws_json(&mut stream).await;
    assert_eq!(msg["type"], "catchup");
    assert!(msg["current_chunks"].is_array());
    assert!(msg["tasks"].is_array());
    assert!(msg["decisions"].is_array());
    assert!(msg["participants"].is_array());
}

// ═══════════════════════════════════════════════════════════════════════════════
// Scenario 10: Chunk broadcast to other participants but not sender
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn s10_chunk_broadcast_not_echoed() {
    let srv = TestServer::start().await;
    let tag = Uuid::new_v4();

    let (admin_id, admin_token) = srv.create_account(&format!("bcast-admin-{tag}")).await;
    let (user_id, user_token) = srv.create_account(&format!("bcast-user-{tag}")).await;

    let org_id = srv
        .create_org(
            admin_token,
            admin_id,
            &format!("bcast-{tag}"),
            "COMPANY_ADMIN",
        )
        .await;
    sqlx::query("INSERT INTO org_members (org_id, account_id, role) VALUES ($1, $2, 'MEMBER')")
        .bind(org_id)
        .bind(user_id)
        .execute(&srv.db)
        .await
        .unwrap();

    let meeting_id = srv
        .create_meeting(admin_token, "Broadcast test", &[user_id])
        .await;
    srv.start_meeting(admin_token, meeting_id).await;

    // Admin connects
    let (mut admin_sink, mut admin_stream) = srv.connect_ws(meeting_id, admin_token).await;
    let _ = TestServer::next_ws_json(&mut admin_stream).await; // connected
    let _ = TestServer::next_ws_json(&mut admin_stream).await; // catchup

    // User connects (admin gets participant_joined)
    let (mut _user_sink, mut user_stream) = srv.connect_ws(meeting_id, user_token).await;
    let _ = TestServer::next_ws_json(&mut user_stream).await; // connected
    let _ = TestServer::next_ws_json(&mut user_stream).await; // catchup

    // Drain the participant_joined messages
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let _ = TestServer::drain_ws(&mut admin_stream, std::time::Duration::from_millis(200)).await;

    // Admin sends a chunk
    let chunk = json!({
        "type": "transcript_chunk",
        "text": "Hello from admin",
        "timestamp_ms": 1000,
    });
    admin_sink
        .send(Message::Text(chunk.to_string().into()))
        .await
        .unwrap();

    // User should receive it
    let msg = TestServer::next_ws_json(&mut user_stream).await;
    assert_eq!(msg["type"], "transcript_chunk");
    assert_eq!(msg["text"], "Hello from admin");

    // Admin should NOT receive their own chunk (within 500ms)
    let admin_msgs =
        TestServer::drain_ws(&mut admin_stream, std::time::Duration::from_millis(500)).await;
    assert!(
        !admin_msgs
            .iter()
            .any(|m| m["type"] == "transcript_chunk" && m["text"] == "Hello from admin"),
        "sender should not receive own chunk"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Scenario 11: Chunk persisted to DB
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn s11_chunk_persisted_to_db() {
    let srv = TestServer::start().await;
    let tag = Uuid::new_v4();

    let (admin_id, admin_token) = srv.create_account(&format!("persist-{tag}")).await;
    srv.create_org(
        admin_token,
        admin_id,
        &format!("persist-{tag}"),
        "COMPANY_ADMIN",
    )
    .await;

    let meeting_id = srv.create_meeting(admin_token, "Persist test", &[]).await;
    srv.start_meeting(admin_token, meeting_id).await;

    let (mut sink, mut stream) = srv.connect_ws(meeting_id, admin_token).await;
    let _ = TestServer::next_ws_json(&mut stream).await; // connected
    let _ = TestServer::next_ws_json(&mut stream).await; // catchup

    // Send 3 chunks
    for i in 0..3 {
        let chunk = json!({
            "type": "transcript_chunk",
            "text": format!("chunk number {i}"),
            "timestamp_ms": 1000 + i * 100,
        });
        sink.send(Message::Text(chunk.to_string().into()))
            .await
            .unwrap();
    }

    // Wait for DB writes
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM transcript_chunks WHERE meeting_id = $1")
            .bind(meeting_id)
            .fetch_one(&srv.db)
            .await
            .unwrap();

    assert_eq!(count, 3, "expected 3 chunks in DB");
}

// ═══════════════════════════════════════════════════════════════════════════════
// Scenario 12: Slot creation on first chunk
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn s12_slot_created_on_first_chunk() {
    let srv = TestServer::start().await;
    let tag = Uuid::new_v4();

    let (admin_id, admin_token) = srv.create_account(&format!("slot-{tag}")).await;
    srv.create_org(
        admin_token,
        admin_id,
        &format!("slot-{tag}"),
        "COMPANY_ADMIN",
    )
    .await;

    let meeting_id = srv.create_meeting(admin_token, "Slot test", &[]).await;
    srv.start_meeting(admin_token, meeting_id).await;

    // Before WS — no slots
    let slot_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM meeting_slots WHERE meeting_id = $1")
            .bind(meeting_id)
            .fetch_one(&srv.db)
            .await
            .unwrap();
    assert_eq!(slot_count, 0);

    let (mut sink, mut stream) = srv.connect_ws(meeting_id, admin_token).await;
    let _ = TestServer::next_ws_json(&mut stream).await; // connected
    let _ = TestServer::next_ws_json(&mut stream).await; // catchup

    // Send a chunk
    sink.send(Message::Text(
        json!({
            "type": "transcript_chunk",
            "text": "first words",
            "timestamp_ms": 5000,
        })
        .to_string()
        .into(),
    ))
    .await
    .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Now there should be 1 slot with ai_status = 'pending' and end_ms IS NULL
    let row: Option<(String, Option<i64>)> =
        sqlx::query_as("SELECT ai_status, end_ms FROM meeting_slots WHERE meeting_id = $1")
            .bind(meeting_id)
            .fetch_optional(&srv.db)
            .await
            .unwrap();

    let (status, end_ms) = row.expect("slot row should exist");
    assert_eq!(status, "pending");
    assert!(end_ms.is_none(), "open slot should have NULL end_ms");
}

// ═══════════════════════════════════════════════════════════════════════════════
// Scenario 13: Slot closes at 5-minute max cap
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn s13_slot_closes_at_max_cap() {
    let srv = TestServer::start().await;
    let tag = Uuid::new_v4();

    let (admin_id, admin_token) = srv.create_account(&format!("maxcap-{tag}")).await;
    srv.create_org(
        admin_token,
        admin_id,
        &format!("maxcap-{tag}"),
        "COMPANY_ADMIN",
    )
    .await;

    let meeting_id = srv.create_meeting(admin_token, "Max cap test", &[]).await;
    srv.start_meeting(admin_token, meeting_id).await;

    let (mut sink, mut stream) = srv.connect_ws(meeting_id, admin_token).await;
    let _ = TestServer::next_ws_json(&mut stream).await; // connected
    let _ = TestServer::next_ws_json(&mut stream).await; // catchup

    // Send first chunk at t=0
    sink.send(Message::Text(
        json!({
            "type": "transcript_chunk",
            "text": "start of slot",
            "timestamp_ms": 0,
        })
        .to_string()
        .into(),
    ))
    .await
    .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Send second chunk at t=301,000ms (>5 min) — triggers max cap
    sink.send(Message::Text(
        json!({
            "type": "transcript_chunk",
            "text": "past the cap",
            "timestamp_ms": 301_000,
        })
        .to_string()
        .into(),
    ))
    .await
    .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // First slot should be closed (end_ms IS NOT NULL)
    let rows: Vec<(i32, Option<i64>)> = sqlx::query_as(
        "SELECT slot_index, end_ms FROM meeting_slots
          WHERE meeting_id = $1 ORDER BY slot_index",
    )
    .bind(meeting_id)
    .fetch_all(&srv.db)
    .await
    .unwrap();

    assert!(rows.len() >= 1, "should have at least 1 slot");
    let (_, end_ms) = &rows[0];
    assert!(end_ms.is_some(), "first slot should be closed (end_ms set)");
}

// ═══════════════════════════════════════════════════════════════════════════════
// Scenario 14: Late joiner gets catchup with chunks + tasks
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn s14_late_joiner_catchup() {
    let srv = TestServer::start().await;
    let tag = Uuid::new_v4();

    let (admin_id, admin_token) = srv.create_account(&format!("late-admin-{tag}")).await;
    let (late_id, late_token) = srv.create_account(&format!("late-joiner-{tag}")).await;

    let org_id = srv
        .create_org(
            admin_token,
            admin_id,
            &format!("late-{tag}"),
            "COMPANY_ADMIN",
        )
        .await;
    sqlx::query("INSERT INTO org_members (org_id, account_id, role) VALUES ($1, $2, 'MEMBER')")
        .bind(org_id)
        .bind(late_id)
        .execute(&srv.db)
        .await
        .unwrap();

    let meeting_id = srv
        .create_meeting(admin_token, "Late join test", &[late_id])
        .await;
    srv.start_meeting(admin_token, meeting_id).await;

    // Admin connects and sends some chunks
    let (mut admin_sink, mut admin_stream) = srv.connect_ws(meeting_id, admin_token).await;
    let _ = TestServer::next_ws_json(&mut admin_stream).await; // connected
    let _ = TestServer::next_ws_json(&mut admin_stream).await; // catchup

    for i in 0..5 {
        admin_sink
            .send(Message::Text(
                json!({
                    "type": "transcript_chunk",
                    "text": format!("admin said word {i}"),
                    "timestamp_ms": 1000 + i * 500,
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
    }
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Insert a fake task and summary so catchup has content
    sqlx::query("INSERT INTO meeting_tasks (meeting_id, title) VALUES ($1, 'Fix the bug')")
        .bind(meeting_id)
        .execute(&srv.db)
        .await
        .unwrap();

    sqlx::query(
        "INSERT INTO meeting_summaries (meeting_id, summary_text) VALUES ($1, 'Team discussed bug fixes')"
    ).bind(meeting_id).execute(&srv.db).await.unwrap();

    // Late joiner connects
    let (mut _late_sink, mut late_stream) = srv.connect_ws(meeting_id, late_token).await;
    let _ = TestServer::next_ws_json(&mut late_stream).await; // connected

    // Catchup should have the summary, chunks, and task
    let catchup = TestServer::next_ws_json(&mut late_stream).await;
    assert_eq!(catchup["type"], "catchup");
    assert_eq!(catchup["summary"], "Team discussed bug fixes");
    assert!(catchup["current_chunks"].as_array().unwrap().len() >= 5);
    assert!(catchup["tasks"].as_array().unwrap().len() >= 1);
    assert_eq!(catchup["tasks"][0]["title"], "Fix the bug");
}

// ═══════════════════════════════════════════════════════════════════════════════
// Scenario 15: Disconnect → buffer → reconnect → resume → missed chunks sent
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn s15_disconnect_resume_protocol() {
    let srv = TestServer::start().await;
    let tag = Uuid::new_v4();

    let (admin_id, admin_token) = srv.create_account(&format!("resume-admin-{tag}")).await;
    let (user_id, user_token) = srv.create_account(&format!("resume-user-{tag}")).await;

    let org_id = srv
        .create_org(
            admin_token,
            admin_id,
            &format!("resume-{tag}"),
            "COMPANY_ADMIN",
        )
        .await;
    sqlx::query("INSERT INTO org_members (org_id, account_id, role) VALUES ($1, $2, 'MEMBER')")
        .bind(org_id)
        .bind(user_id)
        .execute(&srv.db)
        .await
        .unwrap();

    let meeting_id = srv
        .create_meeting(admin_token, "Resume test", &[user_id])
        .await;
    srv.start_meeting(admin_token, meeting_id).await;

    // Admin connects
    let (mut admin_sink, mut admin_stream) = srv.connect_ws(meeting_id, admin_token).await;
    let _ = TestServer::next_ws_json(&mut admin_stream).await; // connected
    let _ = TestServer::next_ws_json(&mut admin_stream).await; // catchup

    // User connects, receives welcome + catchup
    let (mut user_sink, mut user_stream) = srv.connect_ws(meeting_id, user_token).await;
    let _ = TestServer::next_ws_json(&mut user_stream).await; // connected
    let _ = TestServer::next_ws_json(&mut user_stream).await; // catchup
    // Drain participant_joined messages
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let _ = TestServer::drain_ws(&mut user_stream, std::time::Duration::from_millis(200)).await;

    // Admin sends chunk 1 — user receives it
    admin_sink
        .send(Message::Text(
            json!({
                "type": "transcript_chunk",
                "text": "before disconnect",
                "timestamp_ms": 1000,
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();

    let msg = TestServer::next_ws_json(&mut user_stream).await;
    assert_eq!(msg["type"], "transcript_chunk");
    let last_idx = msg["chunk_index"].as_i64().unwrap();

    // User disconnects
    user_sink.close().await.ok();
    drop(user_stream);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // Admin sends more chunks while user is offline
    for i in 0..3 {
        admin_sink
            .send(Message::Text(
                json!({
                    "type": "transcript_chunk",
                    "text": format!("missed chunk {i}"),
                    "timestamp_ms": 2000 + i * 100,
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
    }
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // User reconnects
    let (mut _user_sink2, mut user_stream2) = srv.connect_ws(meeting_id, user_token).await;
    let _ = TestServer::next_ws_json(&mut user_stream2).await; // connected
    let _ = TestServer::next_ws_json(&mut user_stream2).await; // catchup

    // Send resume with last known chunk_index
    _user_sink2
        .send(Message::Text(
            json!({
                "type": "resume",
                "last_chunk_index": last_idx,
            })
            .to_string()
            .into(),
        ))
        .await
        .unwrap();

    // Should receive the missed chunks
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let msgs =
        TestServer::drain_ws(&mut user_stream2, std::time::Duration::from_millis(1000)).await;

    let missed_chunks: Vec<&Value> = msgs
        .iter()
        .filter(|m| m["type"] == "transcript_chunk")
        .collect();

    assert!(
        missed_chunks.len() >= 3,
        "should receive at least 3 missed chunks, got {}",
        missed_chunks.len()
    );

    // Verify disconnect_count was incremented
    let dc: i32 = sqlx::query_scalar(
        "SELECT disconnect_count FROM meeting_participants
          WHERE meeting_id = $1 AND account_id = $2",
    )
    .bind(meeting_id)
    .bind(user_id)
    .fetch_one(&srv.db)
    .await
    .unwrap();
    assert!(dc >= 1, "disconnect_count should be >= 1");
}

// ═══════════════════════════════════════════════════════════════════════════════
// Scenario 16: 4 participants connect simultaneously
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn s16_four_participants_concurrent() {
    let srv = TestServer::start().await;
    let tag = Uuid::new_v4();

    // Create 4 accounts
    let mut accounts = Vec::new();
    for i in 0..4 {
        let (id, token) = srv.create_account(&format!("multi-{i}-{tag}")).await;
        accounts.push((id, token));
    }

    let (admin_id, admin_token) = accounts[0];
    let org_id = srv
        .create_org(
            admin_token,
            admin_id,
            &format!("multi-{tag}"),
            "COMPANY_ADMIN",
        )
        .await;

    for (id, _) in &accounts[1..] {
        sqlx::query(
            "INSERT INTO org_members (org_id, account_id, role) VALUES ($1, $2, 'MEMBER')
                      ON CONFLICT DO NOTHING",
        )
        .bind(org_id)
        .bind(id)
        .execute(&srv.db)
        .await
        .unwrap();
    }

    let participant_ids: Vec<Uuid> = accounts[1..].iter().map(|(id, _)| *id).collect();
    let meeting_id = srv
        .create_meeting(admin_token, "Multi test", &participant_ids)
        .await;
    srv.start_meeting(admin_token, meeting_id).await;

    // All 4 connect simultaneously
    let mut handles = Vec::new();
    for (_, token) in &accounts {
        let url = srv.ws_url(&format!("/v1/meetings/{meeting_id}/ws?token={token}"));
        handles.push(tokio::spawn(async move {
            let (ws, _) = connect_async(&url).await.expect("connect");
            ws
        }));
    }

    let mut connections = Vec::new();
    for h in handles {
        connections.push(h.await.unwrap().split());
    }

    // Verify all 4 got welcome messages
    for (_sink, stream) in &mut connections {
        let msg = TestServer::next_ws_json(stream).await;
        assert_eq!(msg["type"], "connected");
    }

    // Verify hub knows 4 participants
    let connected = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM meeting_participants
          WHERE meeting_id = $1 AND status = 'joined'",
    )
    .bind(meeting_id)
    .fetch_one(&srv.db)
    .await
    .unwrap();
    assert_eq!(connected, 4);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Scenario 17: Meeting end disconnects WebSocket clients
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn s17_meeting_end_disconnects_ws() {
    let srv = TestServer::start().await;
    let tag = Uuid::new_v4();

    let (admin_id, admin_token) = srv.create_account(&format!("endws-{tag}")).await;
    srv.create_org(
        admin_token,
        admin_id,
        &format!("endws-{tag}"),
        "COMPANY_ADMIN",
    )
    .await;

    let meeting_id = srv.create_meeting(admin_token, "End WS test", &[]).await;
    srv.start_meeting(admin_token, meeting_id).await;

    let (mut _sink, mut stream) = srv.connect_ws(meeting_id, admin_token).await;
    let _ = TestServer::next_ws_json(&mut stream).await; // connected
    let _ = TestServer::next_ws_json(&mut stream).await; // catchup

    // End the meeting via HTTP
    srv.client
        .post(srv.url(&format!("/v1/meetings/{meeting_id}/end")))
        .header("Authorization", format!("Bearer {admin_token}"))
        .send()
        .await
        .unwrap();

    // Client should receive MeetingEnded
    let msgs = TestServer::drain_ws(&mut stream, std::time::Duration::from_secs(2)).await;
    assert!(
        msgs.iter().any(|m| m["type"] == "meeting_ended"),
        "should receive meeting_ended message, got: {msgs:?}"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Scenario 18: Meeting detail includes disconnect_count
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn s18_detail_includes_disconnect_count() {
    let srv = TestServer::start().await;
    let tag = Uuid::new_v4();

    let (admin_id, admin_token) = srv.create_account(&format!("dc-{tag}")).await;
    srv.create_org(admin_token, admin_id, &format!("dc-{tag}"), "COMPANY_ADMIN")
        .await;

    let meeting_id = srv.create_meeting(admin_token, "DC count test", &[]).await;

    // Manually set disconnect_count to verify it shows in detail
    sqlx::query("UPDATE meeting_participants SET disconnect_count = 3 WHERE meeting_id = $1")
        .bind(meeting_id)
        .execute(&srv.db)
        .await
        .unwrap();

    let res = srv
        .client
        .get(srv.url(&format!("/v1/meetings/{meeting_id}")))
        .header("Authorization", format!("Bearer {admin_token}"))
        .send()
        .await
        .unwrap();

    let body: Value = res.json().await.unwrap();
    let p = &body["participants"][0];
    assert_eq!(p["disconnect_count"], 3);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Scenario 19: Interleaved chunks from multiple speakers ordered correctly
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn s19_interleaved_chunks_ordered() {
    let srv = TestServer::start().await;
    let tag = Uuid::new_v4();

    let (admin_id, admin_token) = srv.create_account(&format!("interleave-a-{tag}")).await;
    let (user_id, user_token) = srv.create_account(&format!("interleave-b-{tag}")).await;

    let org_id = srv
        .create_org(
            admin_token,
            admin_id,
            &format!("intlv-{tag}"),
            "COMPANY_ADMIN",
        )
        .await;
    sqlx::query("INSERT INTO org_members (org_id, account_id, role) VALUES ($1, $2, 'MEMBER')")
        .bind(org_id)
        .bind(user_id)
        .execute(&srv.db)
        .await
        .unwrap();

    let meeting_id = srv
        .create_meeting(admin_token, "Interleave test", &[user_id])
        .await;
    srv.start_meeting(admin_token, meeting_id).await;

    let (mut a_sink, mut a_stream) = srv.connect_ws(meeting_id, admin_token).await;
    let _ = TestServer::next_ws_json(&mut a_stream).await; // connected
    let _ = TestServer::next_ws_json(&mut a_stream).await; // catchup

    let (mut b_sink, mut b_stream) = srv.connect_ws(meeting_id, user_token).await;
    let _ = TestServer::next_ws_json(&mut b_stream).await; // connected
    let _ = TestServer::next_ws_json(&mut b_stream).await; // catchup

    // Drain join notifications
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let _ = TestServer::drain_ws(&mut a_stream, std::time::Duration::from_millis(200)).await;
    let _ = TestServer::drain_ws(&mut b_stream, std::time::Duration::from_millis(200)).await;

    // A and B interleave chunks
    a_sink
        .send(Message::Text(
            json!({"type":"transcript_chunk","text":"A1","timestamp_ms":100})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    b_sink
        .send(Message::Text(
            json!({"type":"transcript_chunk","text":"B1","timestamp_ms":200})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    a_sink
        .send(Message::Text(
            json!({"type":"transcript_chunk","text":"A2","timestamp_ms":300})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    b_sink
        .send(Message::Text(
            json!({"type":"transcript_chunk","text":"B2","timestamp_ms":400})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Verify DB has 4 chunks in correct order
    let chunks: Vec<(String, i32)> = sqlx::query_as(
        "SELECT text, chunk_index FROM transcript_chunks
          WHERE meeting_id = $1 ORDER BY chunk_index",
    )
    .bind(meeting_id)
    .fetch_all(&srv.db)
    .await
    .unwrap();

    assert_eq!(chunks.len(), 4);
    // chunk_index should be monotonically increasing
    for i in 1..chunks.len() {
        assert!(
            chunks[i].1 > chunks[i - 1].1,
            "chunk_index should be increasing"
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Scenario 20: Org meeting_creator_roles is configurable
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn s20_configurable_creator_roles() {
    let srv = TestServer::start().await;
    let tag = Uuid::new_v4();

    let (member_id, member_token) = srv.create_account(&format!("cfg-member-{tag}")).await;
    let org_id = srv
        .create_org(member_token, member_id, &format!("cfg-{tag}"), "MEMBER")
        .await;

    // Default roles: ["COMPANY_ADMIN", "MANAGER"] — MEMBER can't create
    let res = srv
        .client
        .post(srv.url("/v1/meetings"))
        .header("Authorization", format!("Bearer {member_token}"))
        .json(&json!({"title": "test", "participant_ids": []}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);

    // Update org to allow MEMBER to create meetings
    sqlx::query("UPDATE orgs SET meeting_creator_roles = $1 WHERE id = $2")
        .bind(json!(["COMPANY_ADMIN", "MANAGER", "MEMBER"]))
        .bind(org_id)
        .execute(&srv.db)
        .await
        .unwrap();

    // Now MEMBER should be able to create
    let (peer_id, _) = srv.create_account(&format!("cfg-peer-{tag}")).await;
    let res = srv
        .client
        .post(srv.url("/v1/meetings"))
        .header("Authorization", format!("Bearer {member_token}"))
        .json(&json!({"title": "member meeting", "participant_ids": [peer_id]}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Scenario 21: Invalid/expired token rejected
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn s21_invalid_token_rejected() {
    let srv = TestServer::start().await;
    let fake_token = Uuid::new_v4();

    let res = srv
        .client
        .get(srv.url("/v1/auth/me"))
        .header("Authorization", format!("Bearer {fake_token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);

    let res = srv
        .client
        .get(srv.url("/v1/auth/me"))
        .header("Authorization", "Bearer not-a-uuid")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);

    let res = srv.client.get(srv.url("/v1/auth/me")).send().await.unwrap();
    assert_eq!(res.status(), 401);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Scenario 22: Empty title rejected when creating meeting
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn s22_empty_title_rejected() {
    let srv = TestServer::start().await;
    let tag = Uuid::new_v4();

    let (account_id, token) = srv.create_account(&format!("empty-{tag}")).await;
    srv.create_org(token, account_id, &format!("empty-{tag}"), "COMPANY_ADMIN")
        .await;

    let res = srv
        .client
        .post(srv.url("/v1/meetings"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&json!({"title": "", "participant_ids": []}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 422);

    let res = srv
        .client
        .post(srv.url("/v1/meetings"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&json!({"title": "   ", "participant_ids": []}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 422);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Scenario 23: Cannot start an already-live meeting
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn s23_cannot_start_live_meeting() {
    let srv = TestServer::start().await;
    let tag = Uuid::new_v4();

    let (account_id, token) = srv.create_account(&format!("double-start-{tag}")).await;
    srv.create_org(token, account_id, &format!("dstart-{tag}"), "COMPANY_ADMIN")
        .await;

    let meeting_id = srv.create_meeting(token, "Already live", &[]).await;
    srv.start_meeting(token, meeting_id).await;

    let res = srv
        .client
        .post(srv.url(&format!("/v1/meetings/{meeting_id}/start")))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 409);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Scenario 24: Cannot end an already-ended meeting
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn s24_cannot_end_ended_meeting() {
    let srv = TestServer::start().await;
    let tag = Uuid::new_v4();

    let (account_id, token) = srv.create_account(&format!("double-end-{tag}")).await;
    srv.create_org(token, account_id, &format!("dend-{tag}"), "COMPANY_ADMIN")
        .await;

    let meeting_id = srv.create_meeting(token, "Will end twice", &[]).await;
    srv.start_meeting(token, meeting_id).await;

    let res = srv
        .client
        .post(srv.url(&format!("/v1/meetings/{meeting_id}/end")))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);

    let res = srv
        .client
        .post(srv.url(&format!("/v1/meetings/{meeting_id}/end")))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 409);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Scenario 25: OpenAI status returns disconnected by default
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn s25_openai_status_disconnected_by_default() {
    let srv = TestServer::start().await;
    let tag = Uuid::new_v4();

    let (account_id, token) = srv.create_account(&format!("oai-{tag}")).await;
    srv.create_org(token, account_id, &format!("oai-{tag}"), "COMPANY_ADMIN")
        .await;

    let res = srv
        .client
        .get(srv.url("/v1/openai/status"))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["connected"], false);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Scenario 26: Sync-to-lark rejected on non-ended meeting
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn s26_sync_rejected_on_non_ended_meeting() {
    let srv = TestServer::start().await;
    let tag = Uuid::new_v4();

    let (account_id, token) = srv.create_account(&format!("sync-{tag}")).await;
    srv.create_org(token, account_id, &format!("sync-{tag}"), "COMPANY_ADMIN")
        .await;

    let meeting_id = srv.create_meeting(token, "Not ended", &[]).await;

    let res = srv
        .client
        .post(srv.url(&format!("/v1/meetings/{meeting_id}/sync-to-lark")))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    let status = res.status().as_u16();
    assert!(
        status == 409 || status == 400 || status == 404,
        "sync should reject non-ended meeting, got {status}"
    );

    srv.start_meeting(token, meeting_id).await;
    let res = srv
        .client
        .post(srv.url(&format!("/v1/meetings/{meeting_id}/sync-to-lark")))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    let status = res.status().as_u16();
    assert!(
        status == 409 || status == 400 || status == 404,
        "sync should reject live meeting, got {status}"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Scenario 27: Sync-to-lark rejected for non-member
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn s27_sync_rejected_for_non_member() {
    let srv = TestServer::start().await;
    let tag = Uuid::new_v4();

    let (admin_id, admin_token) = srv.create_account(&format!("sync-admin-{tag}")).await;
    let (_outsider_id, outsider_token) = srv.create_account(&format!("sync-out-{tag}")).await;

    srv.create_org(
        admin_token,
        admin_id,
        &format!("syncnm-{tag}"),
        "COMPANY_ADMIN",
    )
    .await;

    let meeting_id = srv.create_meeting(admin_token, "Admin meeting", &[]).await;
    srv.start_meeting(admin_token, meeting_id).await;

    srv.client
        .post(srv.url(&format!("/v1/meetings/{meeting_id}/end")))
        .header("Authorization", format!("Bearer {admin_token}"))
        .send()
        .await
        .unwrap();

    let res = srv
        .client
        .post(srv.url(&format!("/v1/meetings/{meeting_id}/sync-to-lark")))
        .header("Authorization", format!("Bearer {outsider_token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Scenario 28: Slot DB state tracks chunk_count, word_count, speaker_ids
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn s28_slot_tracks_counts_and_speakers() {
    let srv = TestServer::start().await;
    let tag = Uuid::new_v4();

    let (admin_id, admin_token) = srv.create_account(&format!("slotcount-a-{tag}")).await;
    let (user_id, user_token) = srv.create_account(&format!("slotcount-b-{tag}")).await;

    let org_id = srv
        .create_org(
            admin_token,
            admin_id,
            &format!("slotcnt-{tag}"),
            "COMPANY_ADMIN",
        )
        .await;
    sqlx::query("INSERT INTO org_members (org_id, account_id, role) VALUES ($1, $2, 'MEMBER')")
        .bind(org_id)
        .bind(user_id)
        .execute(&srv.db)
        .await
        .unwrap();

    let meeting_id = srv
        .create_meeting(admin_token, "Count test", &[user_id])
        .await;
    srv.start_meeting(admin_token, meeting_id).await;

    let (mut a_sink, mut a_stream) = srv.connect_ws(meeting_id, admin_token).await;
    let _ = TestServer::next_ws_json(&mut a_stream).await;
    let _ = TestServer::next_ws_json(&mut a_stream).await;

    let (mut b_sink, mut b_stream) = srv.connect_ws(meeting_id, user_token).await;
    let _ = TestServer::next_ws_json(&mut b_stream).await;
    let _ = TestServer::next_ws_json(&mut b_stream).await;

    // Admin: "hello world" (2 words), User: "hi there friend" (3 words), Admin: "one two three four" (4 words)
    a_sink
        .send(Message::Text(
            json!({"type":"transcript_chunk","text":"hello world","timestamp_ms":100})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    b_sink
        .send(Message::Text(
            json!({"type":"transcript_chunk","text":"hi there friend","timestamp_ms":200})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    a_sink
        .send(Message::Text(
            json!({"type":"transcript_chunk","text":"one two three four","timestamp_ms":300})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();

    // Force-close the slot by sending a chunk >5min later
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    a_sink
        .send(Message::Text(
            json!({"type":"transcript_chunk","text":"way later","timestamp_ms":400_000})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let row: Option<(i32, i32, Vec<Uuid>)> = sqlx::query_as(
        "SELECT chunk_count, word_count, speaker_ids
         FROM meeting_slots WHERE meeting_id = $1 AND end_ms IS NOT NULL
         ORDER BY slot_index LIMIT 1",
    )
    .bind(meeting_id)
    .fetch_optional(&srv.db)
    .await
    .unwrap();

    let (chunk_count, word_count, speaker_ids) = row.expect("closed slot should exist");
    // 4 chunks: "hello world" + "hi there friend" + "one two three four" + "way later" (the cap-trigger chunk is included)
    assert_eq!(
        chunk_count, 4,
        "slot should have 4 chunks (including the cap trigger)"
    );
    // 11 words: 2 + 3 + 4 + 2
    assert_eq!(word_count, 11, "slot should have 11 words (2+3+4+2)");
    assert_eq!(speaker_ids.len(), 2, "slot should have 2 distinct speakers");
    assert!(speaker_ids.contains(&admin_id));
    assert!(speaker_ids.contains(&user_id));
}

// ═══════════════════════════════════════════════════════════════════════════════
// Scenario 29: Meeting end closes any open slot
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn s29_meeting_end_closes_open_slot() {
    let srv = TestServer::start().await;
    let tag = Uuid::new_v4();

    let (admin_id, admin_token) = srv.create_account(&format!("endslot-{tag}")).await;
    srv.create_org(
        admin_token,
        admin_id,
        &format!("endslot-{tag}"),
        "COMPANY_ADMIN",
    )
    .await;

    let meeting_id = srv.create_meeting(admin_token, "End slot test", &[]).await;
    srv.start_meeting(admin_token, meeting_id).await;

    let (mut sink, mut stream) = srv.connect_ws(meeting_id, admin_token).await;
    let _ = TestServer::next_ws_json(&mut stream).await;
    let _ = TestServer::next_ws_json(&mut stream).await;

    sink.send(Message::Text(
        json!({"type":"transcript_chunk","text":"open slot","timestamp_ms":1000})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let open: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM meeting_slots WHERE meeting_id = $1 AND end_ms IS NULL",
    )
    .bind(meeting_id)
    .fetch_one(&srv.db)
    .await
    .unwrap();
    assert_eq!(open, 1, "should have 1 open slot");

    srv.client
        .post(srv.url(&format!("/v1/meetings/{meeting_id}/end")))
        .header("Authorization", format!("Bearer {admin_token}"))
        .send()
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let open_after: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM meeting_slots WHERE meeting_id = $1 AND end_ms IS NULL",
    )
    .bind(meeting_id)
    .fetch_one(&srv.db)
    .await
    .unwrap();
    assert_eq!(open_after, 0, "meeting end should close all open slots");
}

// ═══════════════════════════════════════════════════════════════════════════════
// Scenario 30: Cross-org isolation — user cannot see other org's meetings
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn s30_cross_org_isolation() {
    let srv = TestServer::start().await;
    let tag = Uuid::new_v4();

    let (a_id, a_token) = srv.create_account(&format!("orgA-{tag}")).await;
    srv.create_org(a_token, a_id, &format!("orgA-{tag}"), "COMPANY_ADMIN")
        .await;
    let m_a = srv.create_meeting(a_token, "Org A meeting", &[]).await;

    let (b_id, b_token) = srv.create_account(&format!("orgB-{tag}")).await;
    srv.create_org(b_token, b_id, &format!("orgB-{tag}"), "COMPANY_ADMIN")
        .await;

    let res = srv
        .client
        .get(srv.url(&format!("/v1/meetings/{m_a}")))
        .header("Authorization", format!("Bearer {b_token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404, "cross-org meeting should be invisible");

    let res = srv
        .client
        .get(srv.url("/v1/meetings"))
        .header("Authorization", format!("Bearer {b_token}"))
        .send()
        .await
        .unwrap();
    let body: Value = res.json().await.unwrap();
    let meetings = body["meetings"].as_array().unwrap();
    assert!(
        !meetings.iter().any(|m| m["id"] == m_a.to_string()),
        "org B should not see org A's meetings"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Scenario 31: Diagnostics ingest + admin list
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn s31_diagnostics_ingest_and_admin_list() {
    let srv = TestServer::start().await;
    let tag = Uuid::new_v4();
    let device_id = format!("diag-{tag}");

    let res = srv
        .client
        .post(srv.url("/v1/diagnostics"))
        .header("x-forwarded-for", "203.0.113.10")
        .json(&json!({
            "device_id": device_id,
            "events": [{
                "event_type": "backend.spawn_failed",
                "severity": "error",
                "app_version": "2.3.2",
                "os": "macos",
                "arch": "aarch64",
                "channel": "stable",
                "phase": "starting",
                "context": { "error": "spawn failed", "port": 48484 }
            }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "diagnostics ingest should succeed");
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["accepted"], 1);

    let inserted: (String, String, Value) = sqlx::query_as(
        "SELECT event_type, severity, context
           FROM diagnostics_events
          WHERE device_id = $1
          ORDER BY created_at DESC
          LIMIT 1",
    )
    .bind(&device_id)
    .fetch_one(&srv.db)
    .await
    .unwrap();
    assert_eq!(inserted.0, "backend.spawn_failed");
    assert_eq!(inserted.1, "error");
    assert_eq!(inserted.2["port"], 48484);

    let (_admin_id, admin_token) = srv.create_account(&format!("diag-admin-{tag}")).await;
    let res = srv
        .client
        .get(srv.url("/v1/diagnostics"))
        .header("Authorization", format!("Bearer {admin_token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "admin diagnostics list should succeed");
    let body: Value = res.json().await.unwrap();
    let events = body["events"].as_array().unwrap();
    assert!(
        events.iter().any(|e| e["device_id"] == device_id),
        "admin list should include the ingested diagnostics event"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Scenario 32: Diagnostics drops private dictation context
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn s32_diagnostics_rejects_private_context() {
    let srv = TestServer::start().await;
    let device_id = format!("diag-private-{}", Uuid::new_v4());

    let res = srv
        .client
        .post(srv.url("/v1/diagnostics"))
        .json(&json!({
            "device_id": device_id,
            "events": [{
                "event_type": "voice.finish",
                "severity": "error",
                "context": { "transcript": "private dictated words" }
            }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        200,
        "unsafe diagnostics payload should be accepted at request level"
    );
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["accepted"], 0);

    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM diagnostics_events WHERE device_id = $1")
            .bind(&device_id)
            .fetch_one(&srv.db)
            .await
            .unwrap();
    assert_eq!(count, 0, "private transcript event should not be stored");
}

// ═══════════════════════════════════════════════════════════════════════════════
// Multi-org: cross-workspace IDOR guard
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn s_multi_org_idor_blocks_cross_workspace_clients() {
    unsafe {
        std::env::set_var("MULTI_ORG_ENABLED", "true");
    }
    let srv = TestServer::start().await;
    let suffix = Uuid::new_v4().to_string();
    let (account_id, token) = srv.create_account(&suffix).await;
    let org_a = srv
        .create_org(token, account_id, &format!("{suffix}-a"), "MEMBER")
        .await;
    let org_b = srv
        .create_org(token, account_id, &format!("{suffix}-b"), "MEMBER")
        .await;

    let activate_a = srv
        .client
        .post(srv.url(&format!("/v1/orgs/{org_a}/activate")))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(activate_a.status(), 200, "activate org A failed");

    let cross = srv
        .client
        .get(srv.url(&format!("/v1/orgs/{org_b}/clients")))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        cross.status(),
        403,
        "listing org B while org A is active must be forbidden"
    );

    let activate_b = srv
        .client
        .post(srv.url(&format!("/v1/orgs/{org_b}/activate")))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(activate_b.status(), 200, "activate org B failed");

    let allowed = srv
        .client
        .get(srv.url(&format!("/v1/orgs/{org_b}/clients")))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        allowed.status(),
        200,
        "listing org B after activation should succeed"
    );

    let list = srv
        .client
        .get(srv.url("/v1/orgs"))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(list.status(), 200);
    let body: Value = list.json().await.unwrap();
    let org_ids: Vec<String> = body["orgs"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|o| o["id"].as_str().map(str::to_string))
        .collect();
    assert!(org_ids.contains(&org_a.to_string()));
    assert!(org_ids.contains(&org_b.to_string()));
    assert_eq!(body["active_org_id"].as_str().unwrap(), org_b.to_string());

    unsafe {
        std::env::remove_var("MULTI_ORG_ENABLED");
    }
}
