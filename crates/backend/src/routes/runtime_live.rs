use std::future;
use std::time::Instant;

use axum::{
    extract::{
        Json, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::StatusCode,
    response::IntoResponse,
};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message as WsMessage, client::IntoClientRequest},
};
use tracing::{debug, warn};
use uuid::Uuid;

use crate::{
    AppState, LiveServerRuntimeLatency, LiveServerRuntimeResult, get_prefs_cached,
    put_live_server_runtime_result, store::users,
};

const SERVER_STT_PROBE_ENV: &str = "AIRNOTE_ENABLE_SERVER_STT_PROBE";

fn server_stt_probe_enabled() -> bool {
    matches!(
        std::env::var(SERVER_STT_PROBE_ENV)
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
}

type UpstreamSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
type UpstreamSink = futures::stream::SplitSink<UpstreamSocket, WsMessage>;
type UpstreamStream = futures::stream::SplitStream<UpstreamSocket>;

#[derive(Debug, Serialize)]
pub struct LiveRuntimeConfigResponse {
    pub enabled: bool,
    pub connected: bool,
    pub server_url: Option<String>,
    pub runtime_ws_url: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RuntimeNotificationConfigResponse {
    pub connected: bool,
    pub server_url: Option<String>,
    pub notifications_ws_url: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CacheLiveResultRequest {
    pub client_run_id: String,
    #[serde(default)]
    pub transcript: String,
    #[serde(default)]
    pub output: String,
    #[serde(default)]
    pub model_used: String,
    pub latency_ms: CacheLiveResultLatency,
}

#[derive(Debug, Deserialize)]
pub struct CacheLiveResultLatency {
    #[serde(default)]
    pub stt: i64,
    #[serde(default)]
    pub polish: i64,
    #[serde(default)]
    pub total: i64,
}

pub async fn config(State(state): State<AppState>) -> Json<LiveRuntimeConfigResponse> {
    let prefs = get_prefs_cached(&state.prefs_cache, &state.pool, &state.default_user_id).await;
    let enabled = prefs
        .as_ref()
        .map(|p| p.server_audio_runtime_enabled)
        .unwrap_or(false)
        && server_stt_probe_enabled();
    let user = users::get_user(&state.pool, &state.default_user_id);
    let server_url = user
        .as_ref()
        .and_then(|u| u.enterprise_server_url.clone())
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            user.as_ref()
                .and_then(|u| u.cloud_token.as_ref())
                .map(|_| "https://airnote.emiactech.com".to_string())
        });
    let runtime_ws_url = match (
        &server_url,
        user.as_ref().and_then(|u| u.cloud_token.as_ref()),
    ) {
        (Some(base_url), Some(token)) if !token.trim().is_empty() => {
            Some(build_upstream_ws_url(base_url, token))
        }
        _ => None,
    };

    Json(LiveRuntimeConfigResponse {
        enabled,
        connected: runtime_ws_url.is_some(),
        server_url,
        runtime_ws_url,
    })
}

pub async fn notifications_config(
    State(state): State<AppState>,
) -> Json<RuntimeNotificationConfigResponse> {
    let user = users::get_user(&state.pool, &state.default_user_id);
    let server_url = user
        .as_ref()
        .and_then(|u| u.enterprise_server_url.clone())
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            user.as_ref()
                .and_then(|u| u.cloud_token.as_ref())
                .map(|_| "https://airnote.emiactech.com".to_string())
        });
    let notifications_ws_url = match (
        &server_url,
        user.as_ref().and_then(|u| u.cloud_token.as_ref()),
    ) {
        (Some(base_url), Some(token)) if !token.trim().is_empty() => {
            Some(build_notification_ws_url(base_url, token))
        }
        _ => None,
    };

    Json(RuntimeNotificationConfigResponse {
        connected: notifications_ws_url.is_some(),
        server_url,
        notifications_ws_url,
    })
}

pub async fn cache_result(
    State(state): State<AppState>,
    Json(body): Json<CacheLiveResultRequest>,
) -> StatusCode {
    if body.client_run_id.trim().is_empty() {
        return StatusCode::UNPROCESSABLE_ENTITY;
    }
    put_live_server_runtime_result(
        &state.live_server_runtime_cache,
        body.client_run_id,
        LiveServerRuntimeResult {
            transcript: body.transcript,
            output: body.output,
            model_used: if body.model_used.trim().is_empty() {
                "server-audio-runtime".to_string()
            } else {
                body.model_used
            },
            latency_ms: LiveServerRuntimeLatency {
                stt: body.latency_ms.stt,
                polish: body.latency_ms.polish,
                total: body.latency_ms.total,
            },
            stored_at: Instant::now(),
        },
    )
    .await;
    StatusCode::NO_CONTENT
}

pub async fn ws(State(state): State<AppState>, ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(move |socket| async move {
        handle_live_ws(state, socket).await;
    })
}

async fn handle_live_ws(state: AppState, socket: WebSocket) {
    let (mut local_sink, mut local_stream) = socket.split();
    let _ = local_sink
        .send(Message::Text(
            json!({
                "type": "runtime.connected",
                "version": 1,
                "proxy": "local_backend_live_runtime"
            })
            .to_string(),
        ))
        .await;

    let mut upstream_sink: Option<UpstreamSink> = None;
    let mut upstream_stream: Option<UpstreamStream> = None;
    let mut active_client_run_id: Option<String> = None;
    let mut latest_transcript = String::new();

    loop {
        tokio::select! {
            local_msg = local_stream.next() => {
                let Some(local_msg) = local_msg else { break };
                let Ok(local_msg) = local_msg else { break };
                match local_msg {
                    Message::Text(text) => {
                        let Ok(mut value) = serde_json::from_str::<Value>(&text) else {
                            let _ = local_sink.send(Message::Text(json!({
                                "type": "runtime.error",
                                "version": 1,
                                "error_kind": "invalid_json",
                                "message": "invalid local runtime proxy JSON"
                            }).to_string())).await;
                            continue;
                        };
                        let msg_type = value.get("type").and_then(Value::as_str).unwrap_or("");
                        match msg_type {
                            "voice.start" => {
                                if upstream_sink.is_some() || upstream_stream.is_some() {
                                    let _ = local_sink.send(Message::Text(json!({
                                        "type": "runtime.error",
                                        "version": 1,
                                        "error_kind": "recording_already_active",
                                        "message": "a live runtime proxy session is already active"
                                    }).to_string())).await;
                                    continue;
                                }

                                let Some(prefs) = get_prefs_cached(
                                    &state.prefs_cache,
                                    &state.pool,
                                    &state.default_user_id,
                                ).await else {
                                    let _ = local_sink.send(Message::Text(json!({
                                        "type": "runtime.error",
                                        "version": 1,
                                        "error_kind": "prefs_unavailable",
                                        "message": "preferences unavailable"
                                    }).to_string())).await;
                                    continue;
                                };
                                if !prefs.server_audio_runtime_enabled {
                                    let _ = local_sink.send(Message::Text(json!({
                                        "type": "runtime.error",
                                        "version": 1,
                                        "error_kind": "server_audio_runtime_disabled",
                                        "message": "server audio runtime is disabled locally"
                                    }).to_string())).await;
                                    continue;
                                }

                                let Some(user) = users::get_user(&state.pool, &state.default_user_id) else {
                                    let _ = local_sink.send(Message::Text(json!({
                                        "type": "runtime.error",
                                        "version": 1,
                                        "error_kind": "local_user_missing",
                                        "message": "local user not found"
                                    }).to_string())).await;
                                    continue;
                                };
                                let Some(token) = user.cloud_token.filter(|t| !t.trim().is_empty()) else {
                                    let _ = local_sink.send(Message::Text(json!({
                                        "type": "runtime.error",
                                        "version": 1,
                                        "error_kind": "server_runtime_signin_required",
                                        "message": "server audio runtime requires AirNote sign-in"
                                    }).to_string())).await;
                                    continue;
                                };
                                let base_url = user
                                    .enterprise_server_url
                                    .filter(|s| !s.trim().is_empty())
                                    .unwrap_or_else(|| "https://airnote.emiactech.com".to_string());

                                let client_run_id = value
                                    .get("run_id")
                                    .and_then(Value::as_str)
                                    .map(str::to_string)
                                    .unwrap_or_else(|| Uuid::new_v4().to_string());
                                active_client_run_id = Some(client_run_id.clone());
                                latest_transcript.clear();

                                if value.get("selected_model").is_none() {
                                    value["selected_model"] = json!(prefs.selected_model);
                                }
                                if value.get("output_language").is_none() {
                                    value["output_language"] = json!(prefs.output_language);
                                }
                                value["run_id"] = json!(client_run_id);
                                value["source"] = json!("local_backend_live_proxy");

                                let connect_start = Instant::now();
                                match connect_upstream_runtime_ws(&base_url, &token).await {
                                    Ok(socket) => {
                                        let (mut sink, stream) = socket.split();
                                        if let Err(e) = sink.send(WsMessage::Text(value.to_string())).await {
                                            let _ = local_sink.send(Message::Text(json!({
                                                "type": "runtime.error",
                                                "version": 1,
                                                "run_id": active_client_run_id.as_deref(),
                                                "error_kind": "upstream_start_failed",
                                                "message": format!("failed to send start to upstream runtime: {e}")
                                            }).to_string())).await;
                                            continue;
                                        }
                                        upstream_sink = Some(sink);
                                        upstream_stream = Some(stream);
                                        debug!(
                                            "[runtime_live] upstream connected run_id={} in {}ms",
                                            active_client_run_id.as_deref().unwrap_or("unknown"),
                                            connect_start.elapsed().as_millis()
                                        );
                                    }
                                    Err(e) => {
                                        let _ = local_sink.send(Message::Text(json!({
                                            "type": "runtime.error",
                                            "version": 1,
                                            "run_id": active_client_run_id.as_deref(),
                                            "error_kind": "upstream_connect_failed",
                                            "message": e
                                        }).to_string())).await;
                                    }
                                }
                            }
                            "audio.end" => {
                                if let Some(sink) = upstream_sink.as_mut() {
                                    let _ = sink.send(WsMessage::Text(text)).await;
                                }
                            }
                            "audio.frame" => {
                                if let Some(sink) = upstream_sink.as_mut() {
                                    let _ = sink.send(WsMessage::Text(text)).await;
                                }
                            }
                            "ping" => {
                                let _ = local_sink
                                    .send(Message::Text(
                                        json!({"type": "pong", "version": 1, "proxy": "local_backend_live_runtime"}).to_string(),
                                    ))
                                    .await;
                            }
                            _ => {
                                if let Some(sink) = upstream_sink.as_mut() {
                                    let _ = sink.send(WsMessage::Text(text)).await;
                                }
                            }
                        }
                    }
                    Message::Binary(bytes) => {
                        if let Some(sink) = upstream_sink.as_mut() {
                            if sink.send(WsMessage::Binary(bytes.to_vec())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
            upstream_msg = async {
                match upstream_stream.as_mut() {
                    Some(stream) => stream.next().await,
                    None => future::pending().await,
                }
            } => {
                let Some(upstream_msg) = upstream_msg else { break };
                match upstream_msg {
                    Ok(WsMessage::Text(text)) => {
                        if let Ok(value) = serde_json::from_str::<Value>(&text) {
                            match value.get("type").and_then(Value::as_str).unwrap_or("") {
                                "transcript.final" => {
                                    if let Some(transcript) = value.get("text").and_then(Value::as_str) {
                                        latest_transcript = transcript.to_string();
                                    }
                                }
                                "runtime.done" => {
                                    let output = value
                                        .get("output")
                                        .and_then(Value::as_str)
                                        .unwrap_or("")
                                        .to_string();
                                    let model_used = value
                                        .get("model_used")
                                        .and_then(Value::as_str)
                                        .or_else(|| value.get("model").and_then(Value::as_str))
                                        .unwrap_or("server-audio-runtime")
                                        .to_string();
                                    let latency = parse_latency(&value);
                                    if let Some(client_run_id) = active_client_run_id.clone() {
                                        put_live_server_runtime_result(
                                            &state.live_server_runtime_cache,
                                            client_run_id,
                                            LiveServerRuntimeResult {
                                                transcript: latest_transcript.clone(),
                                                output: output.clone(),
                                                model_used,
                                                latency_ms: latency,
                                                stored_at: Instant::now(),
                                            },
                                        ).await;
                                    }
                                }
                                _ => {}
                            }
                        }
                        if local_sink.send(Message::Text(text)).await.is_err() {
                            break;
                        }
                    }
                    Ok(WsMessage::Binary(bytes)) => {
                        if local_sink.send(Message::Binary(bytes)).await.is_err() {
                            break;
                        }
                    }
                    Ok(WsMessage::Close(_)) => break,
                    Ok(_) => {}
                    Err(e) => {
                        warn!("[runtime_live] upstream ws error: {e}");
                        let _ = local_sink.send(Message::Text(json!({
                            "type": "runtime.error",
                            "version": 1,
                            "run_id": active_client_run_id.as_deref(),
                            "error_kind": "upstream_read_failed",
                            "message": "upstream runtime websocket closed unexpectedly"
                        }).to_string())).await;
                        break;
                    }
                }
            }
        }
    }
}

async fn connect_upstream_runtime_ws(
    base_url: &str,
    token: &str,
) -> Result<UpstreamSocket, String> {
    let ws_url = build_upstream_ws_url(base_url, token);
    let mut request = ws_url
        .into_client_request()
        .map_err(|e| format!("invalid upstream ws request: {e}"))?;
    request.headers_mut().insert(
        "User-Agent",
        "AirNote local backend live runtime proxy"
            .parse()
            .map_err(|e| format!("invalid upstream user-agent: {e}"))?,
    );
    let (socket, _) = connect_async(request)
        .await
        .map_err(|e| format!("upstream runtime ws connect failed: {e}"))?;
    Ok(socket)
}

fn build_upstream_ws_url(base_url: &str, token: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let ws_base = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        format!("wss://{base}")
    };
    format!("{ws_base}/v1/runtime/voice/ws?token={token}")
}

fn build_notification_ws_url(base_url: &str, token: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let ws_base = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        format!("wss://{base}")
    };
    format!("{ws_base}/v1/runtime/notifications/ws?token={token}")
}

fn parse_latency(value: &Value) -> LiveServerRuntimeLatency {
    let latency = value.get("latency_ms");
    LiveServerRuntimeLatency {
        stt: latency
            .and_then(|v| v.get("stt"))
            .and_then(Value::as_i64)
            .unwrap_or(0),
        polish: latency
            .and_then(|v| v.get("polish"))
            .and_then(Value::as_i64)
            .unwrap_or(0),
        total: latency
            .and_then(|v| v.get("total"))
            .and_then(Value::as_i64)
            .unwrap_or(0),
    }
}
