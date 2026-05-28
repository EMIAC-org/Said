//! Best-effort guardrails for leaked `airnote-backend` processes.
//!
//! The normal owner is `backend::BackendHandle::Drop`. This module covers
//! starts after crashes, stale PID files, and signal/panic paths that might
//! otherwise leave an old daemon behind.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sysinfo::{Pid, ProcessRefreshKind, RefreshKind, System, UpdateKind};
use tracing::{info, warn};

const BACKEND_NAME: &str = "airnote-backend";
const LEGACY_BACKEND_NAME: &str = "said-backend";

fn data_base() -> PathBuf {
    dirs::data_local_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join(".local/share")))
        .unwrap_or_else(|| std::env::temp_dir())
}

pub fn pid_file() -> PathBuf {
    data_base().join("AirNote").join("airnote-backend.pid")
}

fn legacy_pid_file() -> PathBuf {
    data_base().join("Said").join("said-backend.pid")
}

fn pid_files() -> [PathBuf; 2] {
    [pid_file(), legacy_pid_file()]
}

fn is_backend_name(name: &str) -> bool {
    // Windows process names may include `.exe`; normalize so both process-name
    // and executable-path checks handle macOS and Windows consistently.
    let normalized = name.trim_end_matches(".exe");
    normalized == BACKEND_NAME || normalized == LEGACY_BACKEND_NAME
}

fn process_name_matches(process: &sysinfo::Process) -> bool {
    is_backend_name(&process.name().to_string_lossy())
}

fn exe_name_matches(exe: &Path) -> bool {
    exe.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(is_backend_name)
}

pub fn reap_previous() {
    let sys = System::new_with_specifics(
        RefreshKind::new()
            .with_processes(ProcessRefreshKind::new().with_exe(UpdateKind::OnlyIfNotSet)),
    );

    for pid_path in pid_files() {
        if let Some(pid) = read_pid_file(&pid_path) {
            if process_matches_pid(&sys, pid) {
                info!("[backend-guard] reaping previous backend from pid file pid={pid}");
                terminate_pid(pid, Duration::from_secs(1));
            } else {
                info!("[backend-guard] ignoring stale backend pid file pid={pid}");
            }
            let _ = std::fs::remove_file(&pid_path);
        }
    }

    let current_parent = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));

    for (pid, process) in sys.processes() {
        let Some(exe) = process.exe() else { continue };
        if should_reap_process(exe, current_parent.as_deref()) {
            let raw_pid = pid.as_u32();
            info!("[backend-guard] reaping orphan backend pid={raw_pid} exe={exe:?}");
            terminate_pid(raw_pid, Duration::from_secs(1));
        }
    }
}

pub fn write_pid_file(pid: u32) {
    let path = pid_file();
    if let Some(parent) = path.parent() {
        if let Err(err) = std::fs::create_dir_all(parent) {
            warn!("[backend-guard] failed to create pid dir {parent:?}: {err}");
            return;
        }
    }
    if let Err(err) = std::fs::write(&path, pid.to_string()) {
        warn!("[backend-guard] failed to write pid file {path:?}: {err}");
    }
}

pub fn clear_pid_file() {
    for path in pid_files() {
        if let Err(err) = std::fs::remove_file(&path) {
            if err.kind() != std::io::ErrorKind::NotFound {
                warn!("[backend-guard] failed to clear pid file {path:?}: {err}");
            }
        }
    }
}

pub fn kill_from_pid_file() {
    let sys = System::new_with_specifics(
        RefreshKind::new()
            .with_processes(ProcessRefreshKind::new().with_exe(UpdateKind::OnlyIfNotSet)),
    );
    for path in pid_files() {
        let Some(pid) = read_pid_file(&path) else {
            continue;
        };
        if process_matches_pid(&sys, pid) {
            warn!("[backend-guard] panic/signal cleanup killing backend pid={pid}");
            terminate_pid(pid, Duration::from_secs(1));
        }
        let _ = std::fs::remove_file(path);
    }
}

fn read_pid_file(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn process_matches_pid(sys: &System, pid: u32) -> bool {
    let Some(process) = sys.process(Pid::from_u32(pid)) else {
        return false;
    };
    process_name_matches(process) || process.exe().is_some_and(exe_name_matches)
}

fn should_reap_process(exe: &Path, current_parent: Option<&Path>) -> bool {
    if !exe_name_matches(exe) {
        return false;
    }

    if let Some(current_parent) = current_parent {
        if exe.parent() == Some(current_parent) {
            return true;
        }
    }

    // Match dev builds run straight from the cargo target dir, using
    // `MAIN_SEPARATOR` so the check also fires for Windows backslash paths.
    let sep = std::path::MAIN_SEPARATOR;
    let target_debug = format!("{sep}target{sep}debug{sep}");
    let target_release = format!("{sep}target{sep}release{sep}");
    let path = exe.to_string_lossy();
    path.contains(&target_debug) || path.contains(&target_release)
}

fn terminate_pid(pid: u32, graceful_for: Duration) {
    #[cfg(unix)]
    {
        let pid = pid as libc::pid_t;
        unsafe {
            let _ = libc::kill(pid, libc::SIGTERM);
        }

        let deadline = Instant::now() + graceful_for;
        while Instant::now() < deadline {
            if !pid_is_alive(pid) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        unsafe {
            let _ = libc::kill(pid, libc::SIGKILL);
        }
    }
}

#[cfg(unix)]
fn pid_is_alive(pid: libc::pid_t) -> bool {
    let rc = unsafe { libc::kill(pid, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}
