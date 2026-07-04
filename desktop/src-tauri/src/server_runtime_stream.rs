use std::{
    sync::{Mutex, OnceLock},
    time::Duration,
};

use futures::{
    SinkExt, StreamExt,
    stream::{SplitSink, SplitStream},
};
use serde_json::{Value, json};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};
use tracing::{debug, info, warn};

use crate::{api, backend::BackendEndpoint};

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

#[derive(Debug)]
pub enum AudioMirrorCommand {
    Pcm(Vec<u8>),
    Finalize,
}

enum RuntimeWsActorCommand {
    Warm {
        ep: BackendEndpoint,
    },
    Start {
        recording_id: String,
        ep: BackendEndpoint,
        screen_context: Option<String>,
        target_app: Option<String>,
    },
    Pcm {
        recording_id: String,
        bytes: Vec<u8>,
    },
    Finalize {
        recording_id: String,
    },
}

enum LiveRuntimeTarget {
    Direct { ws_url: String },
    Proxy { ws_url: String },
}

type RuntimeSocket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
type RuntimeSink = SplitSink<RuntimeSocket, Message>;
type RuntimeStream = SplitStream<RuntimeSocket>;

static RUNTIME_WS_ACTOR: OnceLock<Mutex<Option<UnboundedSender<RuntimeWsActorCommand>>>> =
    OnceLock::new();

pub fn start_persistent_runtime_connection(ep: BackendEndpoint) {
    if !server_stt_probe_enabled() || runtime_transport() != "ws" {
        return;
    }
    let actor = runtime_ws_actor();
    let _ = actor.send(RuntimeWsActorCommand::Warm { ep });
}

pub fn maybe_spawn_live_audio_mirror(
    recording_id: String,
    ep: BackendEndpoint,
    screen_context: Option<String>,
    target_app: Option<String>,
) -> Option<UnboundedSender<AudioMirrorCommand>> {
    if !server_stt_probe_enabled() {
        return None;
    }

    if runtime_transport() != "ws" {
        return None;
    }

    let (tx, rx) = unbounded_channel();
    let actor = runtime_ws_actor();
    let _ = actor.send(RuntimeWsActorCommand::Start {
        recording_id: recording_id.clone(),
        ep,
        screen_context,
        target_app,
    });
    tauri::async_runtime::spawn(forward_recording_audio_to_runtime_actor(
        recording_id,
        actor,
        rx,
    ));
    Some(tx)
}

fn runtime_transport() -> String {
    std::env::var("AIRNOTE_SERVER_AUDIO_RUNTIME_TRANSPORT")
        .unwrap_or_else(|_| "http".to_string())
        .trim()
        .to_ascii_lowercase()
}

fn runtime_ws_actor() -> UnboundedSender<RuntimeWsActorCommand> {
    let cell = RUNTIME_WS_ACTOR.get_or_init(|| Mutex::new(None));
    let mut guard = cell.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(existing) = guard.as_ref() {
        if !existing.is_closed() {
            return existing.clone();
        }
    }
    let (tx, rx) = unbounded_channel();
    tauri::async_runtime::spawn(runtime_ws_actor_loop(rx));
    *guard = Some(tx.clone());
    tx
}

async fn forward_recording_audio_to_runtime_actor(
    recording_id: String,
    actor: UnboundedSender<RuntimeWsActorCommand>,
    mut rx: UnboundedReceiver<AudioMirrorCommand>,
) {
    while let Some(cmd) = rx.recv().await {
        let send_result = match cmd {
            AudioMirrorCommand::Pcm(bytes) => actor.send(RuntimeWsActorCommand::Pcm {
                recording_id: recording_id.clone(),
                bytes,
            }),
            AudioMirrorCommand::Finalize => actor.send(RuntimeWsActorCommand::Finalize {
                recording_id: recording_id.clone(),
            }),
        };
        if send_result.is_err() {
            break;
        }
    }
}

async fn runtime_ws_actor_loop(mut rx: UnboundedReceiver<RuntimeWsActorCommand>) {
    let mut sink: Option<RuntimeSink> = None;
    let mut stream: Option<RuntimeStream> = None;
    let mut active_target: Option<LiveRuntimeTarget> = None;
    let mut active_ep: Option<BackendEndpoint> = None;
    let mut active_recording_id: Option<String> = None;
    let mut latest_transcript = String::new();
    let mut heartbeat = tokio::time::interval(Duration::from_secs(25));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            maybe_cmd = rx.recv() => {
                let Some(cmd) = maybe_cmd else { break };
                match cmd {
                    RuntimeWsActorCommand::Warm { ep } => {
                        active_ep = Some(ep.clone());
                        if sink.is_none() {
                            match connect_runtime_actor_socket(&ep).await {
                                Ok((new_sink, new_stream, target, connect_ms)) => {
                                    info!(
                                        "[server_runtime_stream] persistent runtime ws warm target={} in {}ms",
                                        target_name(&target),
                                        connect_ms
                                    );
                                    sink = Some(new_sink);
                                    stream = Some(new_stream);
                                    active_target = Some(target);
                                }
                                Err(e) => {
                                    debug!("[server_runtime_stream] persistent runtime warm skipped: {e}");
                                }
                            }
                        }
                    }
                    RuntimeWsActorCommand::Start { recording_id, ep, screen_context, target_app } => {
                        let prefs = match api::get_preferences(&ep).await {
                            Ok(prefs) => prefs,
                            Err(e) => {
                                debug!("[server_runtime_stream] skip live mirror — prefs unavailable: {e}");
                                continue;
                            }
                        };
                        if !prefs.server_audio_runtime_enabled {
                            debug!("[server_runtime_stream] skip live mirror — server audio runtime disabled");
                            continue;
                        }
                        if active_recording_id.is_some() {
                            warn!(
                                "[server_runtime_stream] persistent runtime start ignored — another recording is active"
                            );
                            continue;
                        }
                        active_ep = Some(ep.clone());
                        if sink.is_none() {
                            match connect_runtime_actor_socket(&ep).await {
                                Ok((new_sink, new_stream, target, connect_ms)) => {
                                    info!(
                                        "[server_runtime_stream] persistent runtime ws connected target={} in {}ms",
                                        target_name(&target),
                                        connect_ms
                                    );
                                    sink = Some(new_sink);
                                    stream = Some(new_stream);
                                    active_target = Some(target);
                                }
                                Err(e) => {
                                    warn!("[server_runtime_stream] live mirror connect skipped: {e}");
                                    continue;
                                }
                            }
                        }

                        let start_msg = build_voice_start_message(
                            &recording_id,
                            &prefs.selected_model,
                            &prefs.output_language,
                            &prefs.stt_provider,
                            screen_context,
                            target_app,
                        );
                        if send_runtime_text_with_reconnect(
                            &mut sink,
                            &mut stream,
                            &mut active_target,
                            &ep,
                            start_msg,
                        )
                        .await
                        .is_err()
                        {
                            warn!(
                                "[server_runtime_stream] persistent runtime voice.start failed run_id={}",
                                recording_id
                            );
                            continue;
                        }
                        latest_transcript.clear();
                        active_recording_id = Some(recording_id.clone());
                        info!(
                            "[server_runtime_stream] live mirror started run_id={} target={}",
                            recording_id,
                            active_target.as_ref().map(target_name).unwrap_or("unknown")
                        );
                    }
                    RuntimeWsActorCommand::Pcm { recording_id, bytes } => {
                        if active_recording_id.as_deref() != Some(recording_id.as_str()) {
                            continue;
                        }
                        let Some(active_sink) = sink.as_mut() else { continue };
                        if active_sink.send(Message::Binary(bytes)).await.is_err() {
                            warn!(
                                "[server_runtime_stream] persistent runtime audio send failed run_id={}",
                                recording_id
                            );
                            clear_runtime_socket(&mut sink, &mut stream, &mut active_target);
                            active_recording_id = None;
                        }
                    }
                    RuntimeWsActorCommand::Finalize { recording_id } => {
                        if active_recording_id.as_deref() != Some(recording_id.as_str()) {
                            continue;
                        }
                        let Some(active_sink) = sink.as_mut() else { continue };
                        if active_sink
                            .send(Message::Text(json!({"type": "audio.end", "run_id": recording_id}).to_string()))
                            .await
                            .is_err()
                        {
                            warn!(
                                "[server_runtime_stream] persistent runtime finalize failed run_id={}",
                                recording_id
                            );
                            clear_runtime_socket(&mut sink, &mut stream, &mut active_target);
                            active_recording_id = None;
                        }
                    }
                }
            }
            msg = async {
                match stream.as_mut() {
                    Some(stream) => stream.next().await,
                    None => std::future::pending().await,
                }
            } => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(value) = serde_json::from_str::<Value>(&text) {
                            handle_runtime_actor_text(
                                &mut latest_transcript,
                                &mut active_recording_id,
                                active_ep.as_ref(),
                                active_target.as_ref(),
                                &value,
                            )
                            .await;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        warn!("[server_runtime_stream] persistent runtime ws disconnected");
                        clear_runtime_socket(&mut sink, &mut stream, &mut active_target);
                        active_recording_id = None;
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        warn!("[server_runtime_stream] persistent runtime ws stream error: {e}");
                        clear_runtime_socket(&mut sink, &mut stream, &mut active_target);
                        active_recording_id = None;
                    }
                }
            }
            _ = heartbeat.tick() => {
                if let Some(active_sink) = sink.as_mut() {
                    if active_sink
                        .send(Message::Text(json!({"type":"ping"}).to_string()))
                        .await
                        .is_err()
                    {
                        warn!("[server_runtime_stream] persistent runtime heartbeat failed");
                        clear_runtime_socket(&mut sink, &mut stream, &mut active_target);
                        active_recording_id = None;
                    }
                } else if let Some(ep) = active_ep.clone() {
                    match connect_runtime_actor_socket(&ep).await {
                        Ok((new_sink, new_stream, target, connect_ms)) => {
                            info!(
                                "[server_runtime_stream] persistent runtime ws reconnected target={} in {}ms",
                                target_name(&target),
                                connect_ms
                            );
                            sink = Some(new_sink);
                            stream = Some(new_stream);
                            active_target = Some(target);
                        }
                        Err(e) => {
                            debug!("[server_runtime_stream] persistent runtime reconnect skipped: {e}");
                        }
                    }
                }
            }
        }
    }
}

fn build_voice_start_message(
    recording_id: &str,
    selected_model: &str,
    output_language: &str,
    stt_provider: &str,
    screen_context: Option<String>,
    target_app: Option<String>,
) -> String {
    json!({
        "type": "voice.start",
        "run_id": recording_id,
        "mode": "normal_voice",
        "selected_model": selected_model,
        "output_language": output_language,
        "stt_provider": said_core::stt::resolve_provider_from_pref(stt_provider),
        "source": "desktop_live_mirror",
        "platform": std::env::consts::OS,
        "app_version": option_env!("CARGO_PKG_VERSION"),
        "screen_context": screen_context.map(|s| s.chars().take(500).collect::<String>()),
        "target_app": target_app,
        "safe_vocab_terms": [],
        "audio": {
            "encoding": "linear16",
            "sample_rate": 16000,
            "channels": 1
        }
    })
    .to_string()
}

async fn connect_runtime_actor_socket(
    ep: &BackendEndpoint,
) -> Result<(RuntimeSink, RuntimeStream, LiveRuntimeTarget, u128), String> {
    let target = resolve_live_runtime_target(ep)
        .await
        .ok_or_else(|| "runtime live target unavailable".to_string())?;
    let connect_start = std::time::Instant::now();
    let (socket, active_target) = connect_target(&target, ep).await?;
    let connect_ms = connect_start.elapsed().as_millis();
    let (sink, stream) = socket.split();
    Ok((sink, stream, active_target, connect_ms))
}

async fn send_runtime_text_with_reconnect(
    sink: &mut Option<RuntimeSink>,
    stream: &mut Option<RuntimeStream>,
    target: &mut Option<LiveRuntimeTarget>,
    ep: &BackendEndpoint,
    text: String,
) -> Result<(), String> {
    if let Some(active_sink) = sink.as_mut() {
        if active_sink.send(Message::Text(text.clone())).await.is_ok() {
            return Ok(());
        }
    }

    clear_runtime_socket(sink, stream, target);
    let (new_sink, new_stream, new_target, connect_ms) = connect_runtime_actor_socket(ep).await?;
    info!(
        "[server_runtime_stream] persistent runtime ws upgraded via config fallback target={} in {}ms",
        target_name(&new_target),
        connect_ms
    );
    *sink = Some(new_sink);
    *stream = Some(new_stream);
    *target = Some(new_target);
    sink.as_mut()
        .ok_or_else(|| "runtime sink unavailable after reconnect".to_string())?
        .send(Message::Text(text))
        .await
        .map_err(|e| format!("runtime send failed after reconnect: {e}"))
}

async fn handle_runtime_actor_text(
    latest_transcript: &mut String,
    active_recording_id: &mut Option<String>,
    ep: Option<&BackendEndpoint>,
    target: Option<&LiveRuntimeTarget>,
    value: &Value,
) {
    match value.get("type").and_then(|v| v.as_str()).unwrap_or("") {
        "transcript.final" => {
            if let Some(transcript) = value.get("text").and_then(|v| v.as_str()) {
                *latest_transcript = transcript.to_string();
            }
        }
        "runtime.error" => {
            let run_id = active_recording_id.as_deref().unwrap_or("-");
            let kind = value
                .get("error_kind")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let message = value
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("runtime mirror error");
            warn!(
                "[server_runtime_stream] live mirror runtime error run_id={} kind={} message={}",
                run_id, kind, message
            );
            *active_recording_id = None;
        }
        "runtime.done" => {
            let run_id = active_recording_id.clone().unwrap_or_default();
            if matches!(target, Some(LiveRuntimeTarget::Direct { .. })) {
                if let Some(ep) = ep {
                    if let Err(e) =
                        cache_direct_runtime_result(ep, &run_id, latest_transcript, value).await
                    {
                        warn!(
                            "[server_runtime_stream] failed caching direct runtime result run_id={}: {e}",
                            run_id
                        );
                    }
                }
            }
            info!(
                "[server_runtime_stream] live mirror runtime done run_id={}",
                run_id
            );
            latest_transcript.clear();
            *active_recording_id = None;
        }
        "runtime.connected" | "pong" | "transcript.partial" | "runtime.status" => {}
        other => {
            debug!("[server_runtime_stream] live mirror event type={other}");
        }
    }
}

fn clear_runtime_socket(
    sink: &mut Option<RuntimeSink>,
    stream: &mut Option<RuntimeStream>,
    target: &mut Option<LiveRuntimeTarget>,
) {
    *sink = None;
    *stream = None;
    *target = None;
}

async fn resolve_live_runtime_target(ep: &BackendEndpoint) -> Option<LiveRuntimeTarget> {
    match api::get_runtime_live_config(ep).await {
        Ok(cfg) if cfg.enabled && cfg.connected => {
            if let Some(ws_url) = cfg.runtime_ws_url.filter(|s| !s.trim().is_empty()) {
                return Some(LiveRuntimeTarget::Direct { ws_url });
            }
        }
        Ok(_) => {
            debug!(
                "[server_runtime_stream] runtime live config unavailable for direct connect — falling back to local proxy"
            );
        }
        Err(e) => {
            debug!(
                "[server_runtime_stream] runtime live config fetch failed — falling back to local proxy: {e}"
            );
        }
    }

    Some(LiveRuntimeTarget::Proxy {
        ws_url: build_local_runtime_ws_url(&ep.url),
    })
}

async fn connect_target(
    target: &LiveRuntimeTarget,
    ep: &BackendEndpoint,
) -> Result<
    (
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        LiveRuntimeTarget,
    ),
    String,
> {
    let request = build_request(target, ep)?;
    match connect_async(request).await {
        Ok((socket, _)) => Ok((socket, clone_target(target))),
        Err(e) => {
            if matches!(target, LiveRuntimeTarget::Direct { .. }) {
                let fallback = LiveRuntimeTarget::Proxy {
                    ws_url: build_local_runtime_ws_url(&ep.url),
                };
                let fallback_request = build_request(&fallback, ep)?;
                match connect_async(fallback_request).await {
                    Ok((socket, _)) => {
                        info!(
                            "[server_runtime_stream] live mirror falling back to local proxy transport"
                        );
                        Ok((socket, fallback))
                    }
                    Err(fallback_err) => Err(format!(
                        "direct connect failed: {e}; proxy fallback failed: {fallback_err}"
                    )),
                }
            } else {
                Err(format!("proxy connect failed: {e}"))
            }
        }
    }
}

fn build_request(
    target: &LiveRuntimeTarget,
    ep: &BackendEndpoint,
) -> Result<tokio_tungstenite::tungstenite::handshake::client::Request, String> {
    let mut request = match target {
        LiveRuntimeTarget::Direct { ws_url } => ws_url.clone(),
        LiveRuntimeTarget::Proxy { ws_url } => ws_url.clone(),
    }
    .into_client_request()
    .map_err(|e| format!("invalid runtime ws request: {e}"))?;

    request.headers_mut().insert(
        "User-Agent",
        "AirNote desktop live runtime mirror"
            .parse()
            .map_err(|e| format!("invalid runtime user-agent: {e}"))?,
    );
    if matches!(target, LiveRuntimeTarget::Proxy { .. }) {
        request.headers_mut().insert(
            "Authorization",
            ep.bearer()
                .parse()
                .map_err(|e| format!("invalid backend auth header: {e}"))?,
        );
    }
    Ok(request)
}

async fn cache_direct_runtime_result(
    ep: &BackendEndpoint,
    recording_id: &str,
    latest_transcript: &str,
    value: &Value,
) -> Result<(), String> {
    let output = value
        .get("output")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let model_used = value
        .get("model_used")
        .and_then(|v| v.as_str())
        .unwrap_or("server-audio-runtime")
        .to_string();
    let latency = value.get("latency_ms");
    api::cache_runtime_live_result(
        ep,
        api::RuntimeLiveResultUpload {
            client_run_id: recording_id.to_string(),
            transcript: latest_transcript.to_string(),
            output,
            model_used,
            latency_ms: api::RuntimeLiveLatency {
                stt: latency
                    .and_then(|v| v.get("stt"))
                    .and_then(|v| v.as_i64())
                    .unwrap_or_default(),
                polish: latency
                    .and_then(|v| v.get("polish"))
                    .and_then(|v| v.as_i64())
                    .unwrap_or_default(),
                total: latency
                    .and_then(|v| v.get("total"))
                    .and_then(|v| v.as_i64())
                    .unwrap_or_default(),
            },
        },
    )
    .await
}

fn clone_target(target: &LiveRuntimeTarget) -> LiveRuntimeTarget {
    match target {
        LiveRuntimeTarget::Direct { ws_url } => LiveRuntimeTarget::Direct {
            ws_url: ws_url.clone(),
        },
        LiveRuntimeTarget::Proxy { ws_url } => LiveRuntimeTarget::Proxy {
            ws_url: ws_url.clone(),
        },
    }
}

fn target_name(target: &LiveRuntimeTarget) -> &'static str {
    match target {
        LiveRuntimeTarget::Direct { .. } => "direct-server-ws",
        LiveRuntimeTarget::Proxy { .. } => "local-proxy-ws",
    }
}

fn build_local_runtime_ws_url(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}/v1/runtime/live/ws")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}/v1/runtime/live/ws")
    } else {
        format!("ws://{base}/v1/runtime/live/ws")
    }
}
