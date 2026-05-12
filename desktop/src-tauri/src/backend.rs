//! Backend daemon lifecycle management.
//!
//! Spawns `said-backend` at Tauri startup, polls health, and exposes
//! the URL + shared secret to the rest of the app.
//!
//! ## Cross-platform process control
//!
//! | Concern        | Unix (macOS)                          | Windows                                  |
//! |----------------|----------------------------------------|------------------------------------------|
//! | Detach group   | `pre_exec(setsid)`                    | `CREATE_NEW_PROCESS_GROUP`               |
//! | Hide console   | n/a (no console window)               | `CREATE_NO_WINDOW`                       |
//! | Graceful stop  | `kill(-pid, SIGTERM)` then `SIGKILL`  | close stdin → `GenerateConsoleCtrlEvent` |
//! | Force stop     | `kill(-pid, SIGKILL)` + `child.kill`  | `child.kill()` (TerminateProcess)        |
//!
//! On Windows the backend is signalled to shut down by closing its stdin
//! handle (see `start_stdin_close_watcher` in `crates/backend/src/main.rs`).
//! `GenerateConsoleCtrlEvent` is also sent as a belt-and-suspenders fallback
//! for backends started outside the new-console-group path.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

#[cfg(unix)]
extern crate libc;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(windows)]
use std::os::windows::process::CommandExt;

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

        // Step 1 — graceful: ask the child to exit on its own terms.
        // Unix: SIGTERM to the process group. Windows: close stdin (the
        // backend's stdin watcher exits cleanly on EOF) and send
        // CTRL_BREAK_EVENT as a fallback for backends not honoring stdin.
        graceful_terminate(&mut self.child, pid);

        // Step 2 — wait up to 3 s for the child to actually exit.
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            if let Ok(Some(_)) = self.child.try_wait() {
                info!("[backend] daemon exited cleanly");
                return;
            }
            if std::time::Instant::now() >= deadline {
                warn!("[backend] graceful shutdown timed out — force killing");
                force_terminate(&mut self.child, pid);
                let _ = self.child.wait();
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

#[cfg(unix)]
fn graceful_terminate(child: &mut Child, pid: u32) {
    // SAFETY: pid was obtained from `child.id()`. SIGTERM to the negative
    // pid targets the process group created via setsid.
    unsafe {
        libc::kill(-(pid as libc::pid_t), libc::SIGTERM);
        libc::kill(pid as libc::pid_t, libc::SIGTERM);
    }
    let _ = child;
}

#[cfg(unix)]
fn force_terminate(child: &mut Child, pid: u32) {
    // SAFETY: pid was obtained from `child.id()`.
    unsafe {
        libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
    }
    let _ = child.kill();
}

#[cfg(windows)]
fn graceful_terminate(child: &mut Child, pid: u32) {
    // Closing stdin causes the backend's stdin-close watcher (in
    // crates/backend/src/main.rs) to exit cleanly.
    drop(child.stdin.take());

    // Belt-and-suspenders: also send CTRL_BREAK_EVENT to the new process
    // group we created at spawn time. Only the group leader receives it,
    // matching the SIGTERM-to-process-group semantics on Unix.
    //
    // SAFETY: GenerateConsoleCtrlEvent is a stable Win32 API; returning 0
    // on failure is informational (we still wait + force-kill below).
    #[allow(unsafe_code)]
    unsafe {
        // 1 == CTRL_BREAK_EVENT
        let _ = generate_console_ctrl_event(1, pid);
    }
}

#[cfg(windows)]
fn force_terminate(child: &mut Child, _pid: u32) {
    // child.kill() invokes TerminateProcess; harsh but final.
    let _ = child.kill();
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    /// Win32 `GenerateConsoleCtrlEvent`. Sends Ctrl-Break / Ctrl-C to all
    /// processes in the given group. `dwCtrlEvent`: 0=CTRL_C, 1=CTRL_BREAK.
    /// `dwProcessGroupId`: 0 = current group, non-zero = target group ID
    /// (matches the pid of the leader if created with CREATE_NEW_PROCESS_GROUP).
    fn GenerateConsoleCtrlEvent(dwCtrlEvent: u32, dwProcessGroupId: u32) -> i32;
}

#[cfg(windows)]
#[inline]
unsafe fn generate_console_ctrl_event(event: u32, pid: u32) -> i32 {
    unsafe { GenerateConsoleCtrlEvent(event, pid) }
}

// ── Spawn ─────────────────────────────────────────────────────────────────────

/// Spawn the backend daemon and return a handle once it is healthy.
///
/// Binary resolution order (first existing path wins):
///   1. Sibling of current exe — bundled .app / .msi (Tauri `externalBin`)
///   2. Walk up from exe — covers `target/debug/` and `target/release/`
///   3. Explicit workspace CWD paths (fallback for `cargo run`)
///
/// On Windows the lookup tries `said-backend.exe` first, then bare
/// `said-backend` for cross-compile output paths that retain the unix name.
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
        )
        // Keep stdin open as the graceful-shutdown channel on Windows
        // (closing the pipe → backend stdin watcher exits cleanly).
        // Inheriting on Unix is harmless; we still use SIGTERM for shutdown.
        .stdin(Stdio::piped());

    spawn_detached(&mut command);

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

#[cfg(unix)]
fn spawn_detached(command: &mut Command) {
    // SAFETY: pre_exec runs in the child between fork() and execve(). setsid
    // is async-signal-safe and the only call we make there.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(windows)]
fn spawn_detached(command: &mut Command) {
    // CREATE_NEW_PROCESS_GROUP makes pid the group leader, so we can target
    // it with GenerateConsoleCtrlEvent on shutdown. CREATE_NO_WINDOW
    // suppresses the otherwise-visible black console flash on each launch.
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
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

/// Candidate filenames for the backend binary in resolution priority order.
///
/// Windows: `.exe` first (native build), then bare name (rare — only if a
/// cross-compile or manual rename produced an unsuffixed exe).
/// Unix: bare name only.
fn binary_filenames() -> &'static [&'static str] {
    #[cfg(windows)]
    {
        &["said-backend.exe", "said-backend"]
    }
    #[cfg(not(windows))]
    {
        &["said-backend"]
    }
}

/// Locate the `said-backend` (or `said-backend.exe`) binary.
fn find_binary() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot get exe path: {e}"))?;
    let names = binary_filenames();

    let mut candidates: Vec<PathBuf> = Vec::new();

    // ── 1. Bundled app: exe is Contents/MacOS/<exe> (mac) or alongside the
    //       .exe (Windows). Tauri `externalBin` strips the target triple.
    if let Some(exe_dir) = exe.parent() {
        for n in names {
            candidates.push(exe_dir.join(n));
        }
    }

    // ── 2. Walk up from exe directory — covers target/debug, target/release
    //       and target/<triple>/{debug,release} layouts
    let mut dir = exe.parent().map(|p| p.to_path_buf());
    for _ in 0..8 {
        if let Some(ref d) = dir {
            for n in names {
                candidates.push(d.join("debug").join(n));
                candidates.push(d.join("release").join(n));
                candidates.push(d.join(n));
            }
            dir = d.parent().map(|p| p.to_path_buf());
        }
    }

    // ── 3. Explicit workspace-relative paths for `cargo tauri dev`
    if let Ok(cwd) = std::env::current_dir() {
        for n in names {
            candidates.push(cwd.join("target").join("debug").join(n));
            candidates.push(cwd.join("target").join("release").join(n));
        }
    }

    candidates.into_iter().find(|p| p.exists()).ok_or_else(|| {
        "said-backend binary not found — run `cargo build -p said-backend --release` first".into()
    })
}
