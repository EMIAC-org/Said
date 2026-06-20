//! Child-process lifecycle for the local Swift STT inference server.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use tracing::{info, warn};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(120);
const HEALTH_POLL: Duration = Duration::from_millis(250);

struct EngineState {
    child: Option<Child>,
    ws_port: u16,
    health_port: u16,
}

impl EngineState {
    fn new() -> Self {
        Self {
            child: None,
            ws_port: 0,
            health_port: 0,
        }
    }
}

fn state() -> &'static Mutex<EngineState> {
    static CELL: OnceLock<Mutex<EngineState>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(EngineState::new()))
}

pub fn ws_port() -> Option<u16> {
    let guard = state().lock().ok()?;
    if guard.ws_port > 0 && guard.child.is_some() {
        Some(guard.ws_port)
    } else {
        None
    }
}

pub fn is_ready() -> bool {
    let Ok(guard) = state().lock() else {
        return false;
    };
    guard.child.is_some() && guard.health_port > 0 && health_ok(guard.health_port)
}

/// Ensure the Swift STT sidecar is running and healthy. Returns the WS port.
pub fn ensure_running() -> Result<u16, String> {
    if !crate::swift_model::is_installed() {
        return Err("Swift model is not installed".to_string());
    }
    let mut guard = state()
        .lock()
        .map_err(|_| "swift engine state poisoned".to_string())?;
    if let Some(child) = guard.child.as_mut() {
        if child.try_wait().ok().flatten().is_some() {
            warn!("[swift_stt] child exited — restarting");
            guard.child = None;
            guard.ws_port = 0;
            guard.health_port = 0;
        } else if guard.ws_port > 0 && health_ok(guard.health_port) {
            return Ok(guard.ws_port);
        }
    }
    let (ws_port, _health_port) = spawn_child(&mut guard)?;
    Ok(ws_port)
}

pub fn shutdown() {
    let mut guard = match state().lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    if let Some(mut child) = guard.child.take() {
        let pid = child.id();
        info!("[swift_stt] shutting down sidecar pid={pid}");
        let _ = child.kill();
        let _ = child.wait();
    }
    guard.ws_port = 0;
    guard.health_port = 0;
}

fn spawn_child(guard: &mut EngineState) -> Result<(u16, u16), String> {
    let script = sidecar_script_path()
        .ok_or_else(|| "Swift STT sidecar script not found (reinstall AirNote)".to_string())?;
    let python = python_binary(&script)?;
    let model_dir = crate::swift_model::model_dir();
    let ws_port = free_port()?;
    let health_port = free_port()?;

    info!("[swift_stt] spawning {python:?} {script:?} model={model_dir:?} port={ws_port}");

    let mut child = Command::new(&python)
        .arg(&script)
        .arg("--model-dir")
        .arg(&model_dir)
        .arg("--port")
        .arg(ws_port.to_string())
        .arg("--health-port")
        .arg(health_port.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to spawn Swift STT sidecar: {e}"))?;

    let deadline = Instant::now() + STARTUP_TIMEOUT;
    while Instant::now() < deadline {
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("Swift STT sidecar exited early: {status}"));
        }
        if health_ok(health_port) {
            guard.child = Some(child);
            guard.ws_port = ws_port;
            guard.health_port = health_port;
            info!("[swift_stt] sidecar ready ws_port={ws_port}");
            return Ok((ws_port, health_port));
        }
        std::thread::sleep(HEALTH_POLL);
    }
    let _ = child.kill();
    Err(
        "Swift STT sidecar did not become healthy in time (install sidecar requirements)"
            .to_string(),
    )
}

fn health_ok(port: u16) -> bool {
    let url = format!("http://127.0.0.1:{port}/health");
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_millis(500))
        .build()
        .ok()
        .and_then(|c| c.get(&url).send().ok())
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

fn sidecar_script_path() -> Option<PathBuf> {
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("resources")
        .join("swift-stt-sidecar")
        .join("server.py");
    if dev.is_file() {
        return Some(dev);
    }
    // Packaged app: tauri.conf.json lists the sidecar as
    // `resources/swift-stt-sidecar/server.py`, and Tauri's bundler preserves
    // that relative path, so the file lands at
    //   Contents/Resources/resources/swift-stt-sidecar/server.py
    // (note the nested lowercase `resources/`). Probe the nested layout first,
    // then the flat one, so we resolve the script regardless of how a given
    // build config places it.
    let exe = std::env::current_exe().ok()?;
    let bundle_resources = exe.parent()?.parent()?.join("Resources");
    let candidates = [
        bundle_resources
            .join("resources")
            .join("swift-stt-sidecar")
            .join("server.py"),
        bundle_resources.join("swift-stt-sidecar").join("server.py"),
    ];
    candidates.into_iter().find(|p| p.is_file())
}

fn python_binary(script: &PathBuf) -> Result<PathBuf, String> {
    if let Ok(custom) = std::env::var("AIRNOTE_SWIFT_PYTHON") {
        let path = PathBuf::from(custom);
        if path.is_file() {
            return Ok(path);
        }
    }
    let venv = script
        .parent()
        .map(|p| p.join(".venv").join("bin").join("python3"));
    if let Some(py) = venv {
        if py.is_file() {
            return Ok(py);
        }
    }
    which_python3()
}

fn which_python3() -> Result<PathBuf, String> {
    let output = Command::new("which")
        .arg("python3")
        .output()
        .map_err(|e| format!("python3 not found: {e}"))?;
    if !output.status.success() {
        return Err(
            "python3 not found — install Python 3 and sidecar requirements (see swift-stt-sidecar README)"
                .to_string(),
        );
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if path.is_empty() {
        return Err("python3 not found".to_string());
    }
    Ok(PathBuf::from(path))
}

fn free_port() -> Result<u16, String> {
    std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("no free port: {e}"))
        .map(|l| l.local_addr().map(|a| a.port()))
        .and_then(|r| r.map_err(|e| format!("no free port: {e}")))
}
