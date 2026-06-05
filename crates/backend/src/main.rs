use clap::Parser;
use reqwest;
use tracing::info;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::prelude::*;

#[derive(Parser, Debug)]
#[command(name = "airnote-backend", about = "AirNote local daemon")]
struct Cli {
    /// TCP port to listen on
    #[arg(long, default_value = "48484")]
    port: u16,

    /// Path to the SQLite database file (default: platform data-local dir + `VoicePolish/db.sqlite`)
    #[arg(long)]
    db: Option<String>,
}

#[tokio::main]
async fn main() {
    // ── Sentry — must init before the tracing subscriber so the panic hook
    //   it installs runs before any other panic handler. Held until main returns.
    let _sentry_guard = said_core::telemetry::init("airnote-backend");

    // ── Structured logging — platform-appropriate log dir ──────────────────────
    let log_dir = said_backend::paths::log_dir();
    std::fs::create_dir_all(&log_dir).ok();
    let log_path = log_dir.join("backend.log");
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .expect("cannot open backend.log");

    let filter = EnvFilter::from_default_env().add_directive("said_backend=debug".parse().unwrap());
    let file_layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_writer(std::sync::Mutex::new(log_file));
    tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(said_core::telemetry::tracing_layer())
        .with(said_core::reporter::tracing_layer())
        .init();

    start_parent_death_watch();

    // ── Load env vars ─────────────────────────────────────────────────────────
    said_core::load_env();

    const DEFAULT_DIAGNOSTICS_BASE: &str = "https://airnote.emiactech.com";
    let diagnostics_base = std::env::var("AIRNOTE_DIAGNOSTICS_URL")
        .or_else(|_| std::env::var("AIRNOTE_CONTROL_PLANE_URL"))
        .unwrap_or_else(|_| DEFAULT_DIAGNOSTICS_BASE.to_string());
    said_core::reporter::configure(&diagnostics_base);
    said_core::reporter::set_phase("backend_starting");

    let cli = Cli::parse();

    // ── Resolve DB path ───────────────────────────────────────────────────────
    let db_path = if let Some(ref path) = cli.db {
        std::path::PathBuf::from(path)
    } else {
        said_backend::store::default_db_path()
    };

    // ── Fingerprint — visible in logs so we can confirm binary version ───────
    info!(
        "airnote-backend build={} features=openai_oauth+codex_api",
        env!("CARGO_PKG_VERSION")
    );

    // ── Open DB + ensure default user ─────────────────────────────────────────
    let pool = said_backend::store::open(&db_path);
    let user_id = said_backend::store::ensure_default_user(&pool);
    let secret = std::env::var("POLISH_SHARED_SECRET").unwrap_or_else(|_| "dev-secret".into());

    let http_client = reqwest::Client::builder()
        .pool_max_idle_per_host(4)
        .pool_idle_timeout(std::time::Duration::from_secs(90))
        .build()
        .expect("failed to build shared HTTP client");

    let wd = std::sync::Arc::new(said_backend::watchdog::WatchdogState::new());

    let state = said_backend::AppState {
        pool: pool.clone(),
        shared_secret: std::sync::Arc::new(secret),
        default_user_id: std::sync::Arc::new(user_id.clone()),
        prefs_cache: std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        lexicon_cache: std::sync::Arc::new(tokio::sync::RwLock::new(None)),
        http_client,
        watchdog: wd.clone(),
    };
    said_backend::routes::vocabulary::spawn_prompt_artifact_repair(state.clone());

    #[cfg(feature = "local-stt")]
    {
        let whisper_model = said_backend::paths::whisper_model_path();
        if whisper_model.is_file() {
            match said_backend::stt::whisper::ensure_model_loaded(
                whisper_model.to_str().unwrap_or_default(),
            ) {
                Ok(()) => info!("whisper model loaded from {}", whisper_model.display()),
                Err(e) => tracing::warn!("whisper model load failed: {e}"),
            }
        } else {
            info!(
                "whisper model not found at {} — local STT unavailable",
                whisper_model.display()
            );
        }
    }

    said_backend::watchdog::spawn_watchdog(pool.clone(), wd, tokio::runtime::Handle::current());

    // ── Build router ──────────────────────────────────────────────────────────
    let router = said_backend::router_with_state(state.clone());

    // ── Bind listener ─────────────────────────────────────────────────────────
    let addr = format!("127.0.0.1:{}", cli.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| panic!("failed to bind {addr}: {e}"));

    info!("airnote-backend listening on http://{addr}");

    // ── 7-day recording + 24h audio file cleanup (every 6 hours) ─────────────
    {
        let pool2 = pool.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(6 * 3600));
            interval.tick().await; // skip first immediate tick
            loop {
                interval.tick().await;
                said_backend::store::history::cleanup_old_recordings(&pool2);
                said_backend::routes::voice::cleanup_old_audio();
                info!("[cleanup] 7-day recording + 24h audio sweep complete");
            }
        });
    }

    // ── Hourly metering report task ───────────────────────────────────────────
    {
        let pool3 = pool.clone();
        let user_id2 = user_id.clone();
        tokio::spawn(async move {
            let cloud_url = std::env::var("CLOUD_API_URL")
                .unwrap_or_else(|_| "https://cloud.voicepolish.app".into());
            let http = reqwest::Client::new();
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
            interval.tick().await; // skip first tick so we don't report on startup
            loop {
                interval.tick().await;
                send_metering_report(&pool3, &user_id2, &http, &cloud_url).await;
            }
        });
    }

    // ── Background alias review lane ────────────────────────────────────────
    {
        let state2 = state.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(15 * 60));
            interval.tick().await; // skip immediate startup run
            loop {
                interval.tick().await;
                said_backend::stt::background::run_pending_alias_reviews(state2.clone(), 12).await;
            }
        });
    }

    // ── Graceful shutdown on SIGTERM / SIGINT ─────────────────────────────────
    let shutdown = async {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            let mut sigterm = signal(SignalKind::terminate()).expect("SIGTERM listener");
            let mut sigint = signal(SignalKind::interrupt()).expect("SIGINT listener");
            tokio::select! {
                _ = sigterm.recv() => info!("received SIGTERM — shutting down"),
                _ = sigint.recv()  => info!("received SIGINT — shutting down"),
            }
        }
        #[cfg(not(unix))]
        {
            tokio::signal::ctrl_c().await.ok();
            info!("received Ctrl-C — shutting down");
        }
    };

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
        .await
        .expect("server error");

    info!("airnote-backend stopped");
}

#[cfg(target_os = "macos")]
fn start_parent_death_watch() {
    let parent_pid = unsafe { libc::getppid() };
    if parent_pid <= 1 {
        return;
    }

    std::thread::spawn(move || unsafe {
        let kq = libc::kqueue();
        if kq == -1 {
            return;
        }

        let mut change = libc::kevent {
            ident: parent_pid as libc::uintptr_t,
            filter: libc::EVFILT_PROC,
            flags: libc::EV_ADD | libc::EV_ENABLE,
            fflags: libc::NOTE_EXIT,
            data: 0,
            udata: std::ptr::null_mut(),
        };
        let mut event = std::mem::zeroed::<libc::kevent>();

        let rc = libc::kevent(kq, &mut change, 1, &mut event, 1, std::ptr::null());
        let _ = libc::close(kq);

        if rc > 0 {
            info!("[parent-watch] parent pid={parent_pid} exited — shutting down backend");
            std::process::exit(0);
        }
    });

    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            if unsafe { libc::getppid() } == 1 {
                info!("[parent-watch] backend reparented to launchd — shutting down");
                std::process::exit(0);
            }
        }
    });
}

#[cfg(not(target_os = "macos"))]
fn start_parent_death_watch() {}

// ── Metering batch ────────────────────────────────────────────────────────────

/// Aggregate recording counts from the last ~24h and POST to the cloud
/// metering endpoint. Silently skips if the user has no cloud token.
async fn send_metering_report(
    pool: &said_backend::store::DbPool,
    user_id: &str,
    http: &reqwest::Client,
    cloud_url: &str,
) {
    use said_backend::store::users;
    use tracing::{debug, warn};

    // Read cloud token
    let Some(user) = users::get_user(pool, user_id) else {
        return;
    };
    let Some(token) = user.cloud_token else {
        debug!("[metering] no cloud token — skipping");
        return;
    };

    let report_base = user
        .enterprise_server_url
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| cloud_url.to_string());

    // Aggregate from recordings over the last 7 days (matches history retention)
    let events: Vec<serde_json::Value> = {
        let conn = match pool.get() {
            Ok(c) => c,
            Err(_) => return,
        };
        let cutoff_ms: i64 = (said_backend::store::now_ms()) - (7 * 24 * 3600 * 1000);

        match conn.prepare(
            "SELECT DATE(datetime(timestamp_ms / 1000, 'unixepoch')) as date,
                    model_used,
                    COUNT(*) as polish_count,
                    SUM(word_count) as word_count
               FROM recordings
              WHERE user_id = ?1 AND timestamp_ms >= ?2
              GROUP BY date, model_used",
        ) {
            Ok(mut stmt) => stmt
                .query_map(rusqlite::params![user_id, cutoff_ms], |row| {
                    let date: String = row.get(0)?;
                    let model: String = row.get(1)?;
                    let polish_count: i64 = row.get(2)?;
                    let word_count: i64 = row.get(3)?;
                    Ok(serde_json::json!({
                        "date":         date,
                        "model":        model,
                        "polish_count": polish_count,
                        "word_count":   word_count,
                    }))
                })
                .ok()
                .map(|rows| rows.flatten().collect())
                .unwrap_or_default(),
            Err(_) => vec![],
        }
    };

    if events.is_empty() {
        debug!("[metering] no events to report");
        return;
    }

    let url = format!("{}/v1/metering/report", report_base.trim_end_matches('/'));
    let payload = serde_json::json!({ "events": events });

    match http
        .post(&url)
        .bearer_auth(&token)
        .json(&payload)
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            info!(
                "[metering] reported {} event rows to {}",
                events.len(),
                report_base
            );
        }
        Ok(resp) => {
            warn!("[metering] cloud returned {}", resp.status());
        }
        Err(e) => {
            debug!("[metering] cloud unreachable: {e}");
        }
    }
}
