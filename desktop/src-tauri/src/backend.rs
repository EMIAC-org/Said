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
    child: Option<Child>,
    /// Windows Job Object handle assigned to the child. Kept alive for the
    /// lifetime of this handle; closing it (Drop or process termination)
    /// triggers `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, which kills the child
    /// even when our own `Drop` doesn't run (Task Manager kill, panic in
    /// the parent, force-uninstall).
    #[cfg(windows)]
    _job: Option<std::os::windows::io::OwnedHandle>,
}

impl BackendHandle {
    pub fn endpoint(&self) -> BackendEndpoint {
        self.endpoint.clone()
    }

    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }
}

impl Drop for BackendHandle {
    fn drop(&mut self) {
        let Some(child) = self.child.as_mut() else {
            info!("[backend] external backend mode — no child process to stop");
            return;
        };
        let pid = child.id();
        info!("[backend] shutting down daemon pid={pid}");
        // SIGTERM → wait 3 s → SIGKILL. The child runs in its own session, so
        // negative PID targets the whole backend process group when available.
        #[cfg(unix)]
        unsafe {
            libc::kill(-(pid as libc::pid_t), libc::SIGTERM);
            libc::kill(pid as libc::pid_t, libc::SIGTERM);
        }
        #[cfg(not(unix))]
        let _ = child.kill();

        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            if let Ok(Some(_)) = child.try_wait() {
                info!("[backend] daemon exited cleanly");
                return;
            }
            if std::time::Instant::now() >= deadline {
                warn!("[backend] graceful shutdown timed out — SIGKILL");
                #[cfg(unix)]
                unsafe {
                    libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
                }
                let _ = child.kill();
                let _ = child.wait();
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
    if let Some(url) = external_backend_url() {
        return connect_external(url);
    }

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

    // Windows: hide the console window that would otherwise pop up for a
    // Rust-built console-subsystem binary, and redirect the child's stdio
    // to null. Without CREATE_NO_WINDOW a black terminal flashes on every
    // launch (the user-visible bug); without the null redirects, stderr
    // from the backend gets eaten by that invisible-or-popped terminal
    // and panics/messages never reach the log file.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
    }

    let child = command
        .spawn()
        .map_err(|e| format!("failed to spawn said-backend ({bin:?}): {e}"))?;

    // Windows: assign the child to a Job Object with KILL_ON_JOB_CLOSE so
    // it's terminated automatically when the desktop process exits — even
    // under Task Manager force-kill or panic-without-unwind. Best-effort:
    // if the assign fails we log and continue; the worst case is one
    // orphan backend, same as today.
    #[cfg(windows)]
    let job_handle = match assign_child_to_kill_on_close_job(&child) {
        Ok(h) => {
            info!("[backend] child assigned to KILL_ON_JOB_CLOSE job");
            Some(h)
        }
        Err(e) => {
            warn!(
                "[backend] failed to assign child to job object: {e} — orphan possible on parent crash"
            );
            None
        }
    };

    let url = format!("http://127.0.0.1:{port}");
    let endpoint = BackendEndpoint {
        url: url.clone(),
        secret,
    };

    // Poll /v1/health until ready — see `health_timeout_ms` for the cold-start
    // headroom that Windows needs on first launch (Defender scan + init).
    poll_health(&url, health_timeout_ms())?;

    info!("[backend] daemon ready at {url}");
    Ok(BackendHandle {
        endpoint,
        child: Some(child),
        #[cfg(windows)]
        _job: job_handle,
    })
}

pub fn external_backend_url() -> Option<String> {
    std::env::var("SAID_EXTERNAL_BACKEND_URL")
        .ok()
        .map(|url| url.trim().trim_end_matches('/').to_string())
        .filter(|url| !url.is_empty())
}

fn connect_external(url: String) -> Result<BackendHandle, String> {
    let secret = std::env::var("POLISH_SHARED_SECRET").unwrap_or_else(|_| "dev-secret".into());

    info!("[backend] using external backend at {url}");
    poll_health(&url, health_timeout_ms())?;
    info!("[backend] external backend ready at {url}");

    Ok(BackendHandle {
        endpoint: BackendEndpoint { url, secret },
        child: None,
        #[cfg(windows)]
        _job: None,
    })
}

// ── Windows: KILL_ON_JOB_CLOSE Job Object for the backend child ──────────────
//
// Without this, force-killing the desktop process (Task Manager, panic,
// uninstaller, parent crash before Drop runs) leaves said-backend.exe alive
// as an orphan — and Windows won't let the next installer overwrite the file
// because it's still locked by that orphan. On Mac the equivalent protection
// is the kqueue-based parent-death watch in crates/backend/src/main.rs:164.
//
// Job Objects do this at the OS level: the parent holds the job handle, and
// when the OS reaps the parent (cleanly OR forcibly) the handle closes, the
// job's refcount hits zero, and the kernel terminates every process inside.

#[cfg(windows)]
fn assign_child_to_kill_on_close_job(
    child: &Child,
) -> Result<std::os::windows::io::OwnedHandle, String> {
    use std::mem;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_BASIC_LIMIT_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation, SetInformationJobObject,
    };

    let child_handle = HANDLE(child.as_raw_handle());

    unsafe {
        let job: HANDLE =
            CreateJobObjectW(None, None).map_err(|e| format!("CreateJobObjectW failed: {e}"))?;

        // Configure the job to kill every assigned process when the job's
        // last open handle is closed.
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
            BasicLimitInformation: JOBOBJECT_BASIC_LIMIT_INFORMATION {
                LimitFlags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                ..mem::zeroed()
            },
            ..mem::zeroed()
        };
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            mem::size_of_val(&info) as u32,
        )
        .map_err(|e| format!("SetInformationJobObject failed: {e}"))?;

        // Assign the already-spawned child to the job. Tiny race window where
        // the child could have spawned its own child before this call, but
        // said-backend never spawns subprocesses, so we're safe in practice.
        AssignProcessToJobObject(job, child_handle)
            .map_err(|e| format!("AssignProcessToJobObject failed: {e}"))?;

        // Wrap the raw HANDLE in OwnedHandle so CloseHandle is called when
        // BackendHandle is dropped. (And so the kernel kills the child if we
        // never drop because we were terminated forcibly.)
        Ok(OwnedHandle::from_raw_handle(job.0 as RawHandle))
    }
}

/// How long to wait for the backend to answer /v1/health before giving up.
///
/// macOS cold-start is sub-second (no AV scan, mature dyld cache). Windows
/// cold-start is dominated by Windows Defender's real-time scan on every
/// previously-unseen .exe, plus SQLite r2d2 pool init and rustls-platform-verifier
/// loading the system cert store — observed ~11 s on Win11 with default
/// Defender settings. Warm starts (after the .exe is in Defender's scan
/// cache) drop to 1-3 s. Pick a budget that accommodates the cold case.
fn health_timeout_ms() -> u64 {
    if cfg!(target_os = "windows") {
        30_000
    } else {
        5_000
    }
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

/// Backend binary filename. `.exe` on Windows; bare name everywhere else.
/// Tauri's externalBin bundling strips the target-triple suffix at bundle
/// time on every OS, so this single name is correct for both dev and
/// bundled lookups.
#[cfg(windows)]
const BACKEND_BIN: &str = "said-backend.exe";
#[cfg(not(windows))]
const BACKEND_BIN: &str = "said-backend";

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
        candidates.push(exe_dir.join(BACKEND_BIN));
    }

    // ── 2. Walk up from exe directory — covers target/debug and target/release layouts
    let mut dir = exe.parent().map(|p| p.to_path_buf());
    for _ in 0..8 {
        if let Some(ref d) = dir {
            candidates.push(d.join("debug").join(BACKEND_BIN));
            candidates.push(d.join("release").join(BACKEND_BIN));
            candidates.push(d.join(BACKEND_BIN));
            dir = d.parent().map(|p| p.to_path_buf());
        }
    }

    // ── 3. Explicit workspace-relative paths for `cargo tauri dev`
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("target").join("debug").join(BACKEND_BIN));
        candidates.push(cwd.join("target").join("release").join(BACKEND_BIN));
    }

    candidates.into_iter().find(|p| p.exists()).ok_or_else(|| {
        "said-backend binary not found — run `cargo build -p said-backend --release` first".into()
    })
}
