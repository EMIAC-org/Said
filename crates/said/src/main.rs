mod learning;

use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use clap::{Parser, Subcommand};
use futures::{SinkExt, StreamExt};
use reqwest::Client;
use said_backend::{
    llm::{openai_codex, prompt::build_user_message},
    stt::deepgram,
};
use said_recorder::{AudioRecorder, ChunkReceiver, SAMPLE_RATE, resample_to_16k};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};
use tracing::{error, warn};
use tracing_subscriber::EnvFilter;
use url::Url;

const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const AUTH_URL: &str = "https://auth.openai.com/oauth/authorize";
const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const REDIRECT_URI: &str = "http://localhost:1455/auth/callback";
const SCOPE: &str = "openid profile email offline_access";
const RESET_SENTINEL: &str = "\u{1F}__RESET__\u{1F}";
const EDIT_WATCH_IDLE_TIMEOUT: Duration = Duration::from_secs(5);
const EDIT_WATCH_MAX_DURATION: Duration = Duration::from_secs(30);
const EDIT_WATCH_FAST_INTERVAL: Duration = Duration::from_millis(30);
const EDIT_WATCH_SLOW_INTERVAL: Duration = Duration::from_millis(200);
const EDIT_WATCH_BLOCKING_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Parser, Debug)]
#[command(name = "voice-polish", about = "Standalone Voice Polish core engine")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Run the hotkey-driven dictation engine.
    Run,
    /// Connect your ChatGPT account and save the token locally for this standalone app.
    Auth,
    /// Show standalone config status.
    Status,
    /// Save or clear the Deepgram API key in the standalone config file.
    DeepgramKey {
        key: Option<String>,
        #[arg(long)]
        clear: bool,
    },
    /// Remove the locally stored OpenAI OAuth token.
    DisconnectOpenai,
    /// Open the macOS permission panes needed for hotkey + paste.
    Permissions,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredOpenAiToken {
    access_token: String,
    refresh_token: Option<String>,
    expires_at: i64,
    connected_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StandaloneConfig {
    deepgram_api_key: Option<String>,
    openai: Option<StoredOpenAiToken>,
    language: String,
    output_language: String,
    tone_preset: String,
    custom_prompt: Option<String>,
}

impl Default for StandaloneConfig {
    fn default() -> Self {
        Self {
            deepgram_api_key: None,
            openai: None,
            language: "auto".into(),
            output_language: "hinglish".into(),
            tone_preset: "neutral".into(),
            custom_prompt: None,
        }
    }
}

#[derive(Clone)]
struct AppCtx {
    config_path: PathBuf,
    http: Client,
}

struct RunnerState {
    recorder: Option<AudioRecorder>,
    transcript_rx: Option<oneshot::Receiver<String>>,
    processing: bool,
    policy: learning::RuntimePolicy,
    watch_generation: u64,
}

#[tokio::main]
async fn main() -> Result<(), String> {
    setup_logging()?;
    let cli = Cli::parse();
    let ctx = open_ctx()?;

    match cli.command.unwrap_or(Commands::Run) {
        Commands::Run => run_listener(ctx).await,
        Commands::Auth => connect_openai(&ctx).await,
        Commands::Status => show_status(&ctx),
        Commands::DeepgramKey { key, clear } => set_deepgram_key(&ctx, key, clear),
        Commands::DisconnectOpenai => disconnect_openai(&ctx),
        Commands::Permissions => {
            said_paster::request_input_monitoring();
            said_paster::request_permission();
            println!("Opened Input Monitoring and Accessibility in System Settings.");
            Ok(())
        }
    }
}

fn setup_logging() -> Result<(), String> {
    let primary = dirs::home_dir()
        .unwrap_or_else(|| ".".into())
        .join("Library")
        .join("Logs")
        .join("VoicePolishStandalone");
    let fallback = std::env::temp_dir().join("VoicePolishStandalone");
    let log_dir = match fs::create_dir_all(&primary) {
        Ok(_) => primary,
        Err(_) => {
            fs::create_dir_all(&fallback).map_err(|e| format!("create log dir failed: {e}"))?;
            fallback
        }
    };
    let log_file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_dir.join("voice-polish.log"))
        .map_err(|e| format!("open log file failed: {e}"))?;

    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive("voice_polish=info".parse().unwrap())
                .add_directive("said_backend=info".parse().unwrap()),
        )
        .with_writer(std::sync::Mutex::new(log_file))
        .init();

    Ok(())
}

fn open_ctx() -> Result<AppCtx, String> {
    let config_path = resolve_config_path()?;
    let http = Client::builder()
        .pool_max_idle_per_host(4)
        .pool_idle_timeout(Duration::from_secs(90))
        .build()
        .map_err(|e| format!("http client init failed: {e}"))?;

    Ok(AppCtx { config_path, http })
}

fn resolve_config_path() -> Result<PathBuf, String> {
    let primary_base = dirs::data_local_dir()
        .or_else(dirs::home_dir)
        .ok_or_else(|| "could not resolve app support directory".to_string())?;
    let primary = primary_base
        .join("VoicePolishStandalone")
        .join("config.json");
    if let Some(parent) = primary.parent() {
        if fs::create_dir_all(parent).is_ok() {
            return Ok(primary);
        }
    }

    let fallback = std::env::temp_dir()
        .join("VoicePolishStandalone")
        .join("config.json");
    if let Some(parent) = fallback.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("config dir create failed: {e}"))?;
    }
    Ok(fallback)
}

fn load_config(ctx: &AppCtx) -> Result<StandaloneConfig, String> {
    if !ctx.config_path.exists() {
        return Ok(StandaloneConfig::default());
    }
    let raw =
        fs::read_to_string(&ctx.config_path).map_err(|e| format!("read config failed: {e}"))?;
    serde_json::from_str(&raw).map_err(|e| format!("parse config failed: {e}"))
}

fn save_config(ctx: &AppCtx, cfg: &StandaloneConfig) -> Result<(), String> {
    if let Some(parent) = ctx.config_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("config dir create failed: {e}"))?;
    }
    let raw =
        serde_json::to_string_pretty(cfg).map_err(|e| format!("serialize config failed: {e}"))?;
    fs::write(&ctx.config_path, raw).map_err(|e| format!("write config failed: {e}"))
}

fn show_status(ctx: &AppCtx) -> Result<(), String> {
    let cfg = load_config(ctx)?;
    let now = now_ms();

    println!("Voice Polish standalone status");
    println!("─────────────────────────────");
    println!("Config    : {}", ctx.config_path.display());
    println!(
        "Deepgram  : {}",
        if cfg
            .deepgram_api_key
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .is_some()
        {
            "configured"
        } else {
            "missing"
        }
    );
    match cfg.openai {
        Some(tok) if tok.expires_at > now => println!("OpenAI    : connected"),
        Some(_) => println!("OpenAI    : expired (run `voice-polish auth`)"),
        None => println!("OpenAI    : missing (run `voice-polish auth`)"),
    }
    println!(
        "Input Mon : {}",
        if said_hotkey::is_input_monitoring_granted() {
            "granted"
        } else {
            "missing"
        }
    );
    println!(
        "AX Paste  : {}",
        if said_paster::is_accessibility_granted() {
            "granted"
        } else {
            "missing"
        }
    );
    Ok(())
}

fn set_deepgram_key(ctx: &AppCtx, key: Option<String>, clear: bool) -> Result<(), String> {
    let mut cfg = load_config(ctx)?;
    if clear {
        cfg.deepgram_api_key = None;
        save_config(ctx, &cfg)?;
        println!("Deepgram key cleared.");
        return Ok(());
    }

    let value = match key {
        Some(v) => v,
        None => prompt_line("Deepgram API key: ")?,
    };
    let trimmed = value.trim().to_string();
    if trimmed.is_empty() {
        return Err("Deepgram key cannot be empty".into());
    }

    cfg.deepgram_api_key = Some(trimmed);
    save_config(ctx, &cfg)?;
    println!("Deepgram key saved.");
    Ok(())
}

fn disconnect_openai(ctx: &AppCtx) -> Result<(), String> {
    let mut cfg = load_config(ctx)?;
    cfg.openai = None;
    save_config(ctx, &cfg)?;
    println!("OpenAI token removed.");
    Ok(())
}

async fn connect_openai(ctx: &AppCtx) -> Result<(), String> {
    let (verifier, challenge) = pkce();
    let state = random_base64url(16);

    let mut url = Url::parse(AUTH_URL).map_err(|e| format!("auth url parse failed: {e}"))?;
    {
        let mut qp = url.query_pairs_mut();
        qp.append_pair("client_id", CLIENT_ID);
        qp.append_pair("redirect_uri", REDIRECT_URI);
        qp.append_pair("response_type", "code");
        qp.append_pair("scope", SCOPE);
        qp.append_pair("code_challenge", &challenge);
        qp.append_pair("code_challenge_method", "S256");
        qp.append_pair("state", &state);
        qp.append_pair("id_token_add_organizations", "true");
        qp.append_pair("codex_cli_simplified_flow", "true");
    }

    println!("Open this URL in your browser and complete sign-in:\n");
    println!("{url}\n");
    let _ = Command::new("open").arg(url.as_str()).spawn();

    let callback = prompt_line("Paste the full callback URL here: ")?;
    let parsed = Url::parse(callback.trim()).map_err(|e| format!("bad callback URL: {e}"))?;

    if let Some(err) = parsed
        .query_pairs()
        .find_map(|(k, v)| (k == "error").then(|| v.into_owned()))
    {
        return Err(format!("OAuth error: {err}"));
    }

    let code = parsed
        .query_pairs()
        .find_map(|(k, v)| (k == "code").then(|| v.into_owned()))
        .ok_or_else(|| "callback URL missing code".to_string())?;
    let returned_state = parsed
        .query_pairs()
        .find_map(|(k, v)| (k == "state").then(|| v.into_owned()))
        .ok_or_else(|| "callback URL missing state".to_string())?;

    if returned_state != state {
        return Err("state mismatch".into());
    }

    let payload = serde_json::json!({
        "client_id": CLIENT_ID,
        "code": code,
        "code_verifier": verifier,
        "redirect_uri": REDIRECT_URI,
        "grant_type": "authorization_code",
    });

    let resp = ctx
        .http
        .post(TOKEN_URL)
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("token request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("token error {status}: {body}"));
    }

    let v: Value = resp
        .json()
        .await
        .map_err(|e| format!("token parse failed: {e}"))?;
    let access_token = v["access_token"]
        .as_str()
        .ok_or_else(|| "missing access_token".to_string())?
        .to_string();
    let refresh_token = v["refresh_token"].as_str().map(|s| s.to_string());
    let expires_in = v["expires_in"].as_i64().unwrap_or(864_000);

    let mut cfg = load_config(ctx)?;
    cfg.openai = Some(StoredOpenAiToken {
        access_token,
        refresh_token,
        expires_at: now_ms() + expires_in * 1000,
        connected_at: now_ms(),
    });
    save_config(ctx, &cfg)?;

    println!(
        "OpenAI connected. Token saved to {}",
        ctx.config_path.display()
    );
    Ok(())
}

async fn run_listener(ctx: AppCtx) -> Result<(), String> {
    let cfg = load_config(&ctx)?;
    if cfg
        .deepgram_api_key
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .is_none()
    {
        return Err("Deepgram key missing. Run `voice-polish deepgram-key` first.".into());
    }
    if cfg.openai.is_none() {
        return Err("OpenAI token missing. Run `voice-polish auth` first.".into());
    }

    if let Ok(device) = AudioRecorder::preflight() {
        println!("Mic ready: {device}");
    }
    if !said_hotkey::is_input_monitoring_granted() {
        println!(
            "Input Monitoring is not granted yet. Run `voice-polish permissions`, grant it, then restart."
        );
    }
    if !said_paster::is_accessibility_granted() {
        println!(
            "Accessibility is not granted yet. Run `voice-polish permissions`, grant it, then restart."
        );
    }

    println!("Voice Polish standalone is listening.");
    println!("Hold Caps Lock to record, release to polish and paste.");

    let state = Arc::new(Mutex::new(RunnerState {
        recorder: None,
        transcript_rx: None,
        processing: false,
        policy: learning::RuntimePolicy::new(),
        watch_generation: 0,
    }));
    let rt = tokio::runtime::Handle::current();

    let on_press_state = Arc::clone(&state);
    let on_press_ctx = ctx.clone();
    let on_press_rt = rt.clone();
    let on_press = Arc::new(move || {
        if let Err(e) = start_recording(&on_press_state, &on_press_ctx, &on_press_rt) {
            eprintln!("start failed: {e}");
            warn!("[voice-polish] start failed: {e}");
        }
    });

    let on_release_state = Arc::clone(&state);
    let on_release_ctx = ctx.clone();
    let on_release_rt = rt.clone();
    let on_release = Arc::new(move || {
        if let Err(e) = finish_recording(&on_release_state, &on_release_ctx, &on_release_rt) {
            eprintln!("finish failed: {e}");
            warn!("[voice-polish] finish failed: {e}");
        }
    });

    said_hotkey::start_hold_listener(on_press, on_release);
    tokio::signal::ctrl_c()
        .await
        .map_err(|e| format!("ctrl-c listener failed: {e}"))?;
    println!("\nStopping Voice Polish standalone.");
    Ok(())
}

fn start_recording(
    state: &Arc<Mutex<RunnerState>>,
    ctx: &AppCtx,
    rt: &tokio::runtime::Handle,
) -> Result<(), String> {
    let cfg = load_config(ctx)?;
    let deepgram_key = cfg
        .deepgram_api_key
        .clone()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| "Deepgram key missing".to_string())?;

    let mut guard = state.lock().map_err(|_| "state lock failed".to_string())?;
    if guard.processing || guard.recorder.is_some() {
        return Ok(());
    }
    guard.watch_generation += 1;

    let _ = said_paster::unlock_focused_app_now();

    let mut recorder = AudioRecorder::new();
    recorder.start()?;
    let chunk_recv = recorder.take_chunk_receiver();
    let (tx, rx) = oneshot::channel::<String>();

    if let Some(chunk_recv) = chunk_recv {
        let key = deepgram_key;
        let language = cfg.language.clone();
        rt.spawn(async move {
            let transcript = stream_to_deepgram_ws(chunk_recv, &key, &language)
                .await
                .unwrap_or_default();
            let _ = tx.send(transcript);
        });
    } else {
        let _ = tx.send(String::new());
    }

    guard.transcript_rx = Some(rx);
    guard.recorder = Some(recorder);
    println!("Recording...");
    Ok(())
}

fn finish_recording(
    state: &Arc<Mutex<RunnerState>>,
    ctx: &AppCtx,
    rt: &tokio::runtime::Handle,
) -> Result<(), String> {
    let (wav, rx_opt) = {
        let mut guard = state.lock().map_err(|_| "state lock failed".to_string())?;
        let Some(mut recorder) = guard.recorder.take() else {
            return Ok(());
        };
        let wav = recorder.stop();
        guard.processing = true;
        (wav, guard.transcript_rx.take())
    };

    let state2 = Arc::clone(state);
    let ctx2 = ctx.clone();
    rt.spawn(async move {
        let result = process_recording(ctx2, wav, rx_opt, state2.clone()).await;
        if let Err(e) = result {
            eprintln!("processing failed: {e}");
            error!("[voice-polish] processing failed: {e}");
        }
        if let Ok(mut guard) = state2.lock() {
            guard.processing = false;
        }
    });

    Ok(())
}

async fn process_recording(
    ctx: AppCtx,
    wav: Option<Vec<u8>>,
    rx_opt: Option<oneshot::Receiver<String>>,
    state: Arc<Mutex<RunnerState>>,
) -> Result<(), String> {
    let wav = wav.ok_or_else(|| "no audio captured".to_string())?;
    let cfg = load_config(&ctx)?;
    let deepgram_key = cfg
        .deepgram_api_key
        .clone()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| "Deepgram key missing".to_string())?;

    let transcript = await_transcript_or_batch(&ctx, &cfg, &deepgram_key, wav, rx_opt).await?;
    println!("Transcript: {transcript}");

    let access_token = ensure_openai_access_token(&ctx).await?;
    let policy_block = state
        .lock()
        .ok()
        .and_then(|guard| guard.policy.prompt_block());
    let system_prompt = build_minimal_system_prompt(&cfg, policy_block.as_deref());
    let user_message = build_user_message(&transcript, &cfg.output_language);
    let polished =
        stream_polish_and_paste(&ctx.http, &access_token, &system_prompt, &user_message).await?;
    println!("Polished: {polished}");
    spawn_learning_watch(state, ctx, cfg, transcript, polished);
    Ok(())
}

async fn await_transcript_or_batch(
    ctx: &AppCtx,
    cfg: &StandaloneConfig,
    deepgram_key: &str,
    wav: Vec<u8>,
    rx_opt: Option<oneshot::Receiver<String>>,
) -> Result<String, String> {
    let pre_transcript = if let Some(rx) = rx_opt {
        match tokio::time::timeout(Duration::from_secs(4), rx).await {
            Ok(Ok(t)) if !t.trim().is_empty() => Some(t),
            _ => None,
        }
    } else {
        None
    };

    if let Some(t) = pre_transcript {
        let plain = strip_confidence_markers(&t);
        if !plain.trim().is_empty() {
            return Ok(plain);
        }
    }

    let bias = said_core::deepgram::BiasPackage {
        stt_mode: said_core::deepgram::resolve_stt_mode(&cfg.language),
        ..Default::default()
    };
    let dg = deepgram::transcribe(&ctx.http, deepgram_key, wav, &bias).await?;
    Ok(dg.transcript)
}

async fn ensure_openai_access_token(ctx: &AppCtx) -> Result<String, String> {
    let mut cfg = load_config(ctx)?;
    let tok = cfg
        .openai
        .clone()
        .ok_or_else(|| "OpenAI token missing. Run `voice-polish auth`.".to_string())?;

    let now = now_ms();
    if tok.expires_at == 0 || tok.expires_at > now + 60_000 {
        return Ok(tok.access_token);
    }

    let refresh_token = tok
        .refresh_token
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            "OpenAI token expired and has no refresh token. Run `voice-polish auth`.".to_string()
        })?;

    let refreshed = openai_codex::refresh_token(&ctx.http, &refresh_token).await?;
    cfg.openai = Some(StoredOpenAiToken {
        access_token: refreshed.access_token.clone(),
        refresh_token: Some(refreshed.refresh_token),
        expires_at: refreshed.expires_at_ms,
        connected_at: tok.connected_at,
    });
    save_config(ctx, &cfg)?;
    Ok(refreshed.access_token)
}

async fn stream_polish_and_paste(
    http: &Client,
    access_token: &str,
    system_prompt: &str,
    user_message: &str,
) -> Result<String, String> {
    let (token_tx, mut token_rx) = mpsc::channel::<String>(64);
    let http2 = http.clone();
    let access_token = access_token.to_string();
    let system_prompt = system_prompt.to_string();
    let user_message = user_message.to_string();

    let llm_task = tokio::spawn(async move {
        openai_codex::stream_polish(
            &http2,
            &access_token,
            openai_codex::MODEL_MINI,
            &system_prompt,
            &user_message,
            token_tx,
        )
        .await
    });

    let mut typed_any = false;
    let mut failed_any = false;

    while let Some(token) = token_rx.recv().await {
        if token == RESET_SENTINEL {
            failed_any = true;
            continue;
        }
        match said_paster::type_text(&token) {
            Ok(true) => typed_any = true,
            Ok(false) => failed_any = true,
            Err(e) => {
                failed_any = true;
                warn!("[voice-polish] type_text failed: {e}");
            }
        }
    }

    let result = llm_task
        .await
        .map_err(|e| format!("llm task join failed: {e}"))??;

    if typed_any && failed_any {
        said_paster::paste_replacing(&result.polished)
            .map_err(|e| format!("paste replacing failed: {e}"))?;
    } else if !typed_any {
        said_paster::paste(&result.polished).map_err(|e| format!("paste failed: {e}"))?;
    }

    Ok(result.polished)
}

async fn stream_to_deepgram_ws(
    chunk_recv: ChunkReceiver,
    deepgram_key: &str,
    language: &str,
) -> Option<String> {
    let lang = if language.is_empty() || language == "auto" {
        "hi"
    } else {
        language
    };
    let url = format!(
        "wss://api.deepgram.com/v1/listen?model=nova-3&language={lang}&punctuate=true&encoding=linear16&sample_rate={SAMPLE_RATE}&channels=1&interim_results=true&endpointing=500&utterance_end_ms=1000"
    );

    let mut req = url.into_client_request().ok()?;
    req.headers_mut().insert(
        "Authorization",
        format!("Token {deepgram_key}").parse().ok()?,
    );

    let (ws, _) = connect_async(req).await.ok()?;
    let (mut ws_tx, mut ws_rx) = ws.split();

    let (async_tx, mut async_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(256);
    let native_rate = chunk_recv.native_rate;
    let sync_rx = chunk_recv.rx;

    std::thread::spawn(move || {
        while let Ok(chunk_f32) = sync_rx.recv() {
            let resampled = resample_to_16k(&chunk_f32, native_rate);
            let pcm_bytes: Vec<u8> = resampled
                .iter()
                .flat_map(|&s| ((s.clamp(-1.0, 1.0) * 32_767.0) as i16).to_le_bytes())
                .collect();
            if async_tx.blocking_send(pcm_bytes).is_err() {
                break;
            }
        }
    });

    let mut transcript_parts: Vec<String> = Vec::new();
    let mut keepalive = tokio::time::interval(Duration::from_secs(8));
    keepalive.tick().await;
    loop {
        tokio::select! {
            chunk = async_rx.recv() => {
                match chunk {
                    Some(pcm) => {
                        if ws_tx.send(Message::Binary(pcm)).await.is_err() {
                            return None;
                        }
                        keepalive.reset();
                    }
                    None => {
                        let _ = ws_tx.send(Message::Text(r#"{"type":"CloseStream"}"#.into())).await;
                        break;
                    }
                }
            }
            _ = keepalive.tick() => {
                let _ = ws_tx.send(Message::Text(r#"{"type":"KeepAlive"}"#.into())).await;
            }
            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(v) = serde_json::from_str::<Value>(&text) {
                            if v["type"].as_str().unwrap_or("") == "Results"
                                && v["is_final"].as_bool().unwrap_or(false)
                            {
                                let enriched = enrich_from_words(&v["channel"]["alternatives"][0]["words"]);
                                if !enriched.is_empty() {
                                    transcript_parts.push(enriched);
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                    _ => {}
                }
            }
        }
    }

    // Key released → wait for Deepgram to close (or timeout at 3s).
    let drain_deadline = Instant::now() + Duration::from_millis(3000);
    while Instant::now() < drain_deadline {
        let remaining = drain_deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(remaining, ws_rx.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    if v["type"].as_str().unwrap_or("") == "Results"
                        && v["is_final"].as_bool().unwrap_or(false)
                    {
                        let enriched = enrich_from_words(&v["channel"]["alternatives"][0]["words"]);
                        if !enriched.is_empty() {
                            transcript_parts.push(enriched);
                        }
                    }
                }
            }
            Ok(Some(Ok(Message::Close(_)))) | Ok(None) => break,
            Ok(Some(Err(_))) => break,
            Ok(Some(Ok(_))) => {}
            Err(_) => break,
        }
    }

    let joined = transcript_parts.join(" ").trim().to_string();
    if joined.is_empty() {
        None
    } else {
        Some(joined)
    }
}

fn enrich_from_words(words: &Value) -> String {
    let Some(arr) = words.as_array() else {
        return String::new();
    };
    let mut out = Vec::with_capacity(arr.len());
    for w in arr {
        let display = w["punctuated_word"]
            .as_str()
            .or_else(|| w["word"].as_str())
            .unwrap_or("");
        if display.is_empty() {
            continue;
        }
        let conf = w["confidence"].as_f64().unwrap_or(1.0);
        if conf < 0.85 {
            out.push(format!("[{}?{:.0}%]", display, conf * 100.0));
        } else {
            out.push(display.to_string());
        }
    }
    out.join(" ")
}

fn build_minimal_system_prompt(cfg: &StandaloneConfig, policy_block: Option<&str>) -> String {
    let tone = match cfg.tone_preset.as_str() {
        "professional" => "Tone: professional and clear.",
        "casual" => "Tone: conversational and natural.",
        "assertive" => "Tone: direct and confident.",
        "concise" => "Tone: compact, but do not drop meaning.",
        _ => "Tone: neutral and clear.",
    };

    let persona = cfg
        .custom_prompt
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("You are a careful voice-transcript polisher.");
    let policy = policy_block.unwrap_or("");

    format!(
        "{persona}\n\
         {tone}\n\
         {policy}\n\
         Input is raw voice-to-text. Return one cleaned final version.\n\
         Preserve meaning and nearly all content words. Do not summarize.\n\
         Remove only fillers and stutters like um, uh, matlab, you know, basically, and accidental repeated function words.\n\
         Fix obvious STT mistakes from sentence context.\n\
         Confidence markers like [word?47%] are input-only hints. Replace them with the best word, or with the literal word without brackets if unsure. Never delete the slot entirely.\n\
         Convert spoken email and URL symbols only when context clearly calls for them: at the rate -> @, dot com -> .com, underscore -> _, slash -> /.\n\
         Preserve the speaker's Hindi-English mix unless the output-language reminder in the user message says otherwise.\n\
         Output only the polished text once. No markdown. No commentary. No alternatives."
    )
}

fn strip_confidence_markers(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '[' {
            let mut j = i + 1;
            while j < chars.len() && chars[j] != ']' {
                j += 1;
            }
            if j < chars.len() {
                let inside: String = chars[i + 1..j].iter().collect();
                if let Some((word, _pct)) = inside.rsplit_once('?') {
                    out.push_str(word);
                    i = j + 1;
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn prompt_line(prompt: &str) -> Result<String, String> {
    print!("{prompt}");
    io::stdout()
        .flush()
        .map_err(|e| format!("stdout flush failed: {e}"))?;
    let mut line = String::new();
    io::stdin()
        .read_line(&mut line)
        .map_err(|e| format!("stdin read failed: {e}"))?;
    Ok(line.trim().to_string())
}

fn spawn_learning_watch(
    state: Arc<Mutex<RunnerState>>,
    ctx: AppCtx,
    cfg: StandaloneConfig,
    transcript: String,
    polished: String,
) {
    let watch_id = {
        let mut guard = match state.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        guard.watch_generation += 1;
        guard.watch_generation
    };

    tokio::spawn(async move {
        if let Err(e) =
            watch_for_user_correction(watch_id, state, ctx, cfg, transcript, polished).await
        {
            warn!("[learning] watcher failed: {e}");
        }
    });
}

async fn watch_for_user_correction(
    watch_id: u64,
    state: Arc<Mutex<RunnerState>>,
    ctx: AppCtx,
    cfg: StandaloneConfig,
    transcript: String,
    polished: String,
) -> Result<(), String> {
    tokio::time::sleep(Duration::from_millis(700)).await;
    if !watch_is_current(&state, watch_id) {
        return Ok(());
    }

    let initial_pid = blocking_ax_option("focused_pid initial", said_paster::focused_pid).await;
    let post_paste = {
        let mut val = blocking_ax_option(
            "read_focused_value_first initial",
            said_paster::read_focused_value_first,
        )
        .await
        .unwrap_or_default();
        if val.is_empty() {
            tokio::time::sleep(Duration::from_millis(300)).await;
            if !watch_is_current(&state, watch_id) {
                return Ok(());
            }
            val = blocking_ax_option(
                "read_focused_value_first retry1",
                said_paster::read_focused_value_first,
            )
            .await
            .unwrap_or_default();
        }
        if val.is_empty() {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if !watch_is_current(&state, watch_id) {
                return Ok(());
            }
            val = blocking_ax_option(
                "read_focused_value_first retry2",
                said_paster::read_focused_value_first,
            )
            .await
            .unwrap_or_default();
        }
        val
    };

    let mut last_val = post_paste.clone();
    let mut best_candidate = post_paste.clone();
    let mut idle_at = Instant::now();
    let started = Instant::now();
    let mut last_change_at = Instant::now();
    let mut current_interval = EDIT_WATCH_FAST_INTERVAL;
    let mut last_pid = initial_pid;

    loop {
        tokio::time::sleep(current_interval).await;
        if !watch_is_current(&state, watch_id) {
            return Ok(());
        }

        let now_pid = blocking_ax_option("focused_pid poll", said_paster::focused_pid).await;
        let pid_switched = matches!((initial_pid, now_pid), (Some(a), Some(b)) if a != b);
        if pid_switched {
            break;
        }

        let now_val = if now_pid != last_pid {
            last_pid = now_pid;
            blocking_ax_option(
                "read_focused_value_first focus-change",
                said_paster::read_focused_value_first,
            )
            .await
        } else {
            blocking_ax_option(
                "read_focused_value_fast poll",
                said_paster::read_focused_value_fast,
            )
            .await
        }
        .unwrap_or_default();

        if now_val != last_val {
            idle_at = Instant::now();
            last_change_at = Instant::now();
            current_interval = EDIT_WATCH_FAST_INTERVAL;
            if shares_word_overlap(&now_val, &polished) {
                best_candidate = now_val.clone();
            }
            last_val = now_val;
        } else if last_change_at.elapsed() > Duration::from_secs(2) {
            current_interval = EDIT_WATCH_SLOW_INTERVAL;
        }

        if idle_at.elapsed() > EDIT_WATCH_IDLE_TIMEOUT
            || started.elapsed() > EDIT_WATCH_MAX_DURATION
        {
            break;
        }
    }

    let effective_val = if shares_word_overlap(&last_val, &polished) {
        last_val.clone()
    } else if best_candidate != post_paste {
        best_candidate.clone()
    } else {
        last_val.clone()
    };
    let final_pid = blocking_ax_option("focused_pid final", said_paster::focused_pid).await;

    let user_kept = if !post_paste.is_empty() {
        if effective_val == post_paste {
            log_policy_accept(&state);
            return Ok(());
        }
        extract_kept(&polished, &post_paste, &effective_val)
    } else if matches!((initial_pid, final_pid), (Some(a), Some(b)) if a == b) {
        let captured = tokio::task::spawn_blocking(said_paster::capture_focused_text_via_selection)
            .await
            .map_err(|e| format!("clipboard capture join failed: {e}"))?
            .unwrap_or_default();
        let captured = captured.trim().to_string();
        if captured.is_empty() {
            return Ok(());
        }
        if captured.contains(polished.trim()) {
            extract_kept(polished.trim(), polished.trim(), &captured)
        } else {
            captured
        }
    } else {
        return Ok(());
    };

    if user_kept.trim().is_empty() || user_kept.trim() == polished.trim() {
        log_policy_accept(&state);
        return Ok(());
    }
    if !shares_word_overlap(&user_kept, &polished) && !is_format_transformation(&user_kept) {
        return Ok(());
    }
    if !is_meaningful_edit(&polished, &user_kept) {
        return Ok(());
    }

    let access_token = ensure_openai_access_token(&ctx).await?;
    let Some(observation) = analyze_edit_with_codex(
        &ctx.http,
        &access_token,
        &cfg,
        &transcript,
        &polished,
        &user_kept,
    )
    .await?
    else {
        return Ok(());
    };

    let mut guard = state.lock().map_err(|_| "state lock failed".to_string())?;
    if guard.watch_generation != watch_id {
        return Ok(());
    }
    let outcome = guard.policy.observe(observation.clone());
    let snapshot = guard.policy.snapshot();
    let rule = &guard.policy.candidates()[outcome.candidate_index];
    println!(
        "[learning] {} {:?} candidate: {:?} -> {:?} · evidence={} · why={} · totals: events={} candidates={}",
        if outcome.created_candidate {
            "created"
        } else {
            "updated"
        },
        rule.kind,
        observation.source_span,
        observation.target_span,
        outcome.candidate_evidence,
        observation.why_summary.as_deref().unwrap_or("n/a"),
        snapshot.event_count,
        snapshot.candidate_count,
    );
    Ok(())
}

async fn blocking_ax_option<T, F>(label: &'static str, f: F) -> Option<T>
where
    T: Send + 'static,
    F: FnOnce() -> Option<T> + Send + 'static,
{
    match tokio::time::timeout(EDIT_WATCH_BLOCKING_TIMEOUT, tokio::task::spawn_blocking(f)).await {
        Ok(Ok(value)) => value,
        Ok(Err(err)) => {
            warn!("[learning] blocking watcher task {label} failed: {err}");
            None
        }
        Err(_) => {
            warn!("[learning] blocking watcher task {label} timed out");
            None
        }
    }
}

fn watch_is_current(state: &Arc<Mutex<RunnerState>>, watch_id: u64) -> bool {
    state
        .lock()
        .map(|guard| guard.watch_generation == watch_id)
        .unwrap_or(false)
}

fn log_policy_accept(state: &Arc<Mutex<RunnerState>>) {
    if let Ok(guard) = state.lock() {
        let snapshot = guard.policy.snapshot();
        println!(
            "[learning] accepted · events={} candidates={}",
            snapshot.event_count, snapshot.candidate_count
        );
    }
}

#[derive(Debug, Deserialize)]
struct LearningAnalysis {
    learn: bool,
    error_class: Option<String>,
    why: Option<String>,
    topic_hints: Option<Vec<String>>,
}

async fn analyze_edit_with_codex(
    http: &Client,
    access_token: &str,
    cfg: &StandaloneConfig,
    transcript: &str,
    polished: &str,
    user_kept: &str,
) -> Result<Option<learning::CorrectionObservation>, String> {
    let app_hint = blocking_ax_option("focused_pid app hint", said_paster::focused_pid)
        .await
        .and_then(focused_app_hint_for_pid);

    let Some(mut observation) =
        learning::observation_from_manual_correction(learning::ManualCorrectionInput {
            transcript: transcript.to_string(),
            polished: polished.to_string(),
            corrected: user_kept.to_string(),
            app_hint: app_hint.clone(),
            language_mode: cfg.language.clone(),
            output_language: cfg.output_language.clone(),
            confidence_band: learning::ConfidenceBand::Unknown,
        })
    else {
        return Ok(None);
    };

    let system_prompt = r#"You analyze one user correction made after speech-to-text polishing.
Decide whether the correction is a reusable learning signal for later similar contexts in the same session.
Prioritize understanding why the user changed it.

Learn only compact, local, reusable corrections such as:
- entity, acronym, brand, person, or product spelling fixes
- phrase-level STT corrections
- preserving words the model dropped
- preventing unwanted translation in mixed Hindi-English text
- formatting fixes when the sentence context clearly calls for them

Do not learn:
- whole-sentence rewrites
- broad tone changes
- one-off paraphrases
- edits that do not clearly preserve the same meaning

Return strict minified JSON only with this schema:
{"learn":true|false,"error_class":"stt_entity_error|stt_phrase_error|formatting_preference|style_preference|content_preservation_fix|translation_fix","why":"short reason","topic_hints":["hint1","hint2"]}

If this should not be learned, return:
{"learn":false,"why":"short reason","topic_hints":[]}"#;

    let user_message = format!(
        "App: {}\nLanguage mode: {}\nOutput language: {}\nOriginal transcript: {}\nPolished output: {}\nUser-corrected final text: {}\nChanged source span: {}\nChanged target span: {}\nLeft context: {}\nRight context: {}",
        app_hint.as_deref().unwrap_or("unknown"),
        cfg.language,
        cfg.output_language,
        transcript,
        polished,
        user_kept,
        observation.source_span,
        observation.target_span,
        observation.context.left_context.join(" "),
        observation.context.right_context.join(" "),
    );

    let (token_tx, token_rx) = mpsc::channel::<String>(8);
    drop(token_rx);
    let result = openai_codex::stream_polish(
        http,
        access_token,
        openai_codex::MODEL_MINI,
        system_prompt,
        &user_message,
        token_tx,
    )
    .await?;

    let analysis = parse_learning_analysis(&result.polished).ok_or_else(|| {
        format!(
            "learning analysis returned non-JSON output: {}",
            result.polished.trim()
        )
    })?;

    if !analysis.learn {
        return Ok(None);
    }

    if let Some(error_class) = analysis.error_class.as_deref().and_then(parse_error_class) {
        observation.error_class = error_class;
    }
    if let Some(why_text) = analysis
        .why
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        observation.why_summary = Some(why_text.to_string());
    }
    if let Some(hints) = analysis.topic_hints {
        let mut merged = observation.context.topic_hints.clone();
        for hint in hints {
            let trimmed = hint.trim();
            if trimmed.is_empty() {
                continue;
            }
            if !merged
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(trimmed))
            {
                merged.push(trimmed.to_string());
            }
        }
        observation.context.topic_hints = merged;
    }

    Ok(Some(observation))
}

fn parse_learning_analysis(raw: &str) -> Option<LearningAnalysis> {
    let trimmed = raw.trim();
    if let Ok(parsed) = serde_json::from_str::<LearningAnalysis>(trimmed) {
        return Some(parsed);
    }

    let without_fences = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .map(str::trim)
        .unwrap_or(trimmed)
        .strip_suffix("```")
        .map(str::trim)
        .unwrap_or(trimmed);
    if let Ok(parsed) = serde_json::from_str::<LearningAnalysis>(without_fences) {
        return Some(parsed);
    }

    let start = without_fences.find('{')?;
    let end = without_fences.rfind('}')?;
    serde_json::from_str::<LearningAnalysis>(&without_fences[start..=end]).ok()
}

fn parse_error_class(raw: &str) -> Option<learning::ErrorClass> {
    let norm = raw.trim().to_ascii_lowercase();
    match norm.as_str() {
        "stt_entity_error" => Some(learning::ErrorClass::SttEntityError),
        "stt_phrase_error" => Some(learning::ErrorClass::SttPhraseError),
        "formatting_preference" => Some(learning::ErrorClass::FormattingPreference),
        "style_preference" => Some(learning::ErrorClass::StylePreference),
        "content_preservation_fix" => Some(learning::ErrorClass::ContentPreservationFix),
        "translation_fix" => Some(learning::ErrorClass::TranslationFix),
        _ => None,
    }
}

fn focused_app_hint_for_pid(pid: i32) -> Option<String> {
    if pid <= 0 {
        return None;
    }

    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if raw.is_empty() {
        return None;
    }

    let name = raw
        .rsplit('/')
        .next()
        .unwrap_or(&raw)
        .trim_end_matches(".app")
        .trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_ascii_lowercase())
    }
}

fn is_format_transformation(text: &str) -> bool {
    let t = text.trim();
    if t.contains('@') && t.contains('.') && !t.contains(' ') {
        return true;
    }
    if t.starts_with("http://")
        || t.starts_with("https://")
        || t.starts_with("www.")
        || t.contains("://")
    {
        return true;
    }
    if t.starts_with('@') || (t.contains('_') && !t.contains(' ') && t.len() < 40) {
        return true;
    }
    let digits = t.chars().filter(|c| c.is_ascii_digit()).count();
    if digits >= 7
        && t.chars()
            .all(|c| c.is_ascii_digit() || " -.+()\u{00A0}".contains(c))
    {
        return true;
    }
    false
}

fn shares_word_overlap(candidate: &str, reference: &str) -> bool {
    let ref_words: std::collections::HashSet<String> = reference
        .split_whitespace()
        .filter(|w| w.chars().count() > 3)
        .map(|w| w.to_lowercase())
        .collect();
    if ref_words.is_empty() {
        return !candidate.trim().is_empty();
    }
    candidate
        .split_whitespace()
        .any(|w| ref_words.contains(&w.to_lowercase()))
}

fn extract_kept(polished: &str, post_paste: &str, last_val: &str) -> String {
    let Some(offset) = post_paste.find(polished.trim()) else {
        return last_val.to_string();
    };

    let prefix = &post_paste[..offset];
    let after_end = offset + polished.trim().len();
    let suffix = &post_paste[after_end..];

    if let Some(after_prefix) = last_val.strip_prefix(prefix) {
        if let Some(edited) = after_prefix.strip_suffix(suffix) {
            return edited.trim().to_string();
        }
        return after_prefix.trim().to_string();
    }

    last_val.to_string()
}

fn is_meaningful_edit(polished: &str, user_kept: &str) -> bool {
    let p = normalize_for_diff(polished);
    let k = normalize_for_diff(user_kept);

    if p == k {
        return false;
    }

    let p_words: Vec<&str> = p.split_whitespace().collect();
    let k_words: Vec<&str> = k.split_whitespace().collect();
    let max_len = p_words.len().max(k_words.len());
    let mut word_diffs = 0usize;
    let mut jargon_diff = false;

    for i in 0..max_len {
        let pw = p_words.get(i).copied().unwrap_or("");
        let kw = k_words.get(i).copied().unwrap_or("");
        if pw != kw
            && (pw.chars().any(|c| c.is_alphanumeric()) || kw.chars().any(|c| c.is_alphanumeric()))
        {
            word_diffs += 1;
            if pw.chars().any(|c| c.is_ascii_digit()) || kw.chars().any(|c| c.is_ascii_digit()) {
                jargon_diff = true;
            }
        }
    }

    if word_diffs == 0 {
        return false;
    }

    let char_diff = simple_char_distance(&p, &k);
    let min_char_diff = if jargon_diff { 1 } else { 3 };
    char_diff >= min_char_diff
}

fn normalize_for_diff(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
        .replace('\u{201c}', "\"")
        .replace('\u{201d}', "\"")
        .replace('\u{2018}', "'")
        .replace('\u{2019}', "'")
        .replace('\u{2014}', "-")
        .replace('\u{2013}', "-")
        .replace('\u{2026}', "...")
        .replace('\u{00a0}', " ")
}

fn simple_char_distance(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let min_len = a_chars.len().min(b_chars.len());
    let mut diff = a_chars.len().abs_diff(b_chars.len());
    for i in 0..min_len {
        if a_chars[i] != b_chars[i] {
            diff += 1;
        }
    }
    diff
}

fn pkce() -> (String, String) {
    use sha2::{Digest, Sha256};
    let verifier = random_base64url(32);
    let challenge = base64url_encode(&Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

fn random_base64url(n: usize) -> String {
    let mut hasher = DefaultHasher::new();
    SystemTime::now().hash(&mut hasher);
    std::thread::current().id().hash(&mut hasher);
    let seed = hasher.finish();

    let mut bytes = vec![0u8; n];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = ((seed >> ((i % 8) * 8)) ^ (i as u64)) as u8;
    }
    base64url_encode(&bytes)
}

fn base64url_encode(bytes: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len() * 4 / 3 + 4);
    let mut i = 0;
    while i + 2 < bytes.len() {
        let b0 = bytes[i] as usize;
        let b1 = bytes[i + 1] as usize;
        let b2 = bytes[i + 2] as usize;
        out.push(CHARS[b0 >> 2] as char);
        out.push(CHARS[((b0 & 3) << 4) | (b1 >> 4)] as char);
        out.push(CHARS[((b1 & 0x0f) << 2) | (b2 >> 6)] as char);
        out.push(CHARS[b2 & 0x3f] as char);
        i += 3;
    }
    match bytes.len().saturating_sub(i) {
        1 => {
            let b0 = bytes[i] as usize;
            out.push(CHARS[b0 >> 2] as char);
            out.push(CHARS[(b0 & 3) << 4] as char);
        }
        2 => {
            let b0 = bytes[i] as usize;
            let b1 = bytes[i + 1] as usize;
            out.push(CHARS[b0 >> 2] as char);
            out.push(CHARS[((b0 & 3) << 4) | (b1 >> 4)] as char);
            out.push(CHARS[(b1 & 0x0f) << 2] as char);
        }
        _ => {}
    }
    out
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
