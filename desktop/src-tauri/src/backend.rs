//! Backend daemon lifecycle management.
//!
//! Spawns `said-backend` at Tauri startup, polls health, and exposes
//! the URL + shared secret to the rest of the app.

use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

#[cfg(unix)]
extern crate libc;
#[cfg(unix)]
use std::os::unix::process::CommandExt;

use tracing::{info, warn};

// ── BackendEndpoint — cheap clone for API calls ───────────────────────────────

/// URL + shared secret. Cloned freely; does NOT own the child process.
#[derive(Clone)]
pub struct BackendEndpoint {
    pub url: String,
    pub secret: String,
}

impl BackendEndpoint {
    /// `Authorization: Bearer <secret>` value.
    pub fn bearer(&self) -> String {
        format!("Bearer {}", self.secret)
    }
}

// ── BackendHandle — owns the child process ────────────────────────────────────

/// Full handle returned by `spawn()`. Owns the child process.
pub struct BackendHandle {
    pub endpoint: BackendEndpoint,
    #[allow(dead_code)]
    child: Child,
}

impl BackendHandle {
    pub fn endpoint(&self) -> BackendEndpoint {
        self.endpoint.clone()
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }
}

impl Drop for BackendHandle {
    fn drop(&mut self) {
        let pid = self.child.id();
        info!("[backend] shutting down daemon pid={pid}");
        // SIGTERM → wait 3 s → SIGKILL. The child runs in its own session, so
        // negative PID targets the whole backend process group when available.
        #[cfg(unix)]
        unsafe {
            libc::kill(-(pid as libc::pid_t), libc::SIGTERM);
            libc::kill(pid as libc::pid_t, libc::SIGTERM);
        }
        #[cfg(not(unix))]
        let _ = self.child.kill();

        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            if let Ok(Some(_)) = self.child.try_wait() {
                info!("[backend] daemon exited cleanly");
                return;
            }
            if std::time::Instant::now() >= deadline {
                warn!("[backend] graceful shutdown timed out — SIGKILL");
                #[cfg(unix)]
                unsafe {
                    libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
                }
                let _ = self.child.kill();
                let _ = self.child.wait();
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

// ── Spawn ─────────────────────────────────────────────────────────────────────

/// Spawn the backend daemon and return a handle once it is healthy.
///
/// Binary resolution order (first existing path wins):
///   1. `target/debug/said-backend`        — cargo dev build
///   2. `target/release/said-backend`      — cargo release build
///   3. Sibling of current executable      — bundled in .app
pub fn spawn() -> Result<BackendHandle, String> {
    let secret = uuid::Uuid::new_v4().to_string();
    let port = free_port()?;
    let bin = find_binary()?;

    info!("[backend] spawning {bin:?} on port {port}");

    let mut command = Command::new(&bin);
    command
        .arg("--port")
        .arg(port.to_string())
        .env("POLISH_SHARED_SECRET", &secret)
        // Forward important env vars from the Tauri process
        .env(
            "GATEWAY_API_KEY",
            std::env::var("GATEWAY_API_KEY").unwrap_or_default(),
        )
        .env(
            "DEEPGRAM_API_KEY",
            std::env::var("DEEPGRAM_API_KEY").unwrap_or_default(),
        )
        .env(
            "GEMINI_API_KEY",
            std::env::var("GEMINI_API_KEY").unwrap_or_default(),
        );

    #[cfg(unix)]
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let child = command
        .spawn()
        .map_err(|e| format!("failed to spawn said-backend ({bin:?}): {e}"))?;

    let url = format!("http://127.0.0.1:{port}");
    let endpoint = BackendEndpoint {
        url: url.clone(),
        secret,
    };

    // Poll /v1/health until ready (5 second timeout)
    poll_health(&url, 5_000)?;

    info!("[backend] daemon ready at {url}");
    Ok(BackendHandle { endpoint, child })
}

/// Block until `GET {base_url}/v1/health` returns 2xx or timeout_ms elapses.
fn poll_health(base_url: &str, timeout_ms: u64) -> Result<(), String> {
    let health_url = format!("{base_url}/v1/health");
    let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
    let client = reqwest::blocking::Client::new();

    while std::time::Instant::now() < deadline {
        match client
            .get(&health_url)
            .timeout(Duration::from_millis(300))
            .send()
        {
            Ok(r) if r.status().is_success() => return Ok(()),
            Ok(r) => warn!("[backend] health check got {}", r.status()),
            Err(e) => warn!("[backend] health check error: {e}"),
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    Err(format!(
        "backend did not become healthy within {timeout_ms}ms"
    ))
}

/// Pick a random free TCP port.
fn free_port() -> Result<u16, String> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("failed to bind ephemeral port: {e}"))?;
    Ok(listener.local_addr().unwrap().port())
}

/// Locate the `said-backend` binary.
///
/// Resolution order (first existing path wins):
///   1. Sibling of current exe — bundled .app (Tauri `externalBin`)
///   2. Walk up from exe — covers `target/debug/` and `target/release/`
///   3. Explicit workspace CWD paths (fallback for `cargo run`)
fn find_binary() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot get exe path: {e}"))?;

    let mut candidates: Vec<PathBuf> = Vec::new();

    // ── 1. Bundled app: exe is Contents/MacOS/<exe>, backend is Contents/MacOS/said-backend
    //       (Tauri externalBin strips the target triple suffix in the bundle)
    if let Some(exe_dir) = exe.parent() {
        candidates.push(exe_dir.join("said-backend"));
    }

    // ── 2. Walk up from exe directory — covers target/debug and target/release layouts
    let mut dir = exe.parent().map(|p| p.to_path_buf());
    for _ in 0..8 {
        if let Some(ref d) = dir {
            candidates.push(d.join("debug").join("said-backend"));
            candidates.push(d.join("release").join("said-backend"));
            candidates.push(d.join("said-backend"));
            dir = d.parent().map(|p| p.to_path_buf());
        }
    }

    // ── 3. Explicit workspace-relative paths for `cargo tauri dev`
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("target").join("debug").join("said-backend"));
        candidates.push(cwd.join("target").join("release").join("said-backend"));
    }

    candidates.into_iter().find(|p| p.exists()).ok_or_else(|| {
        "said-backend binary not found — run `cargo build -p said-backend --release` first"
            .into()
    })
}
