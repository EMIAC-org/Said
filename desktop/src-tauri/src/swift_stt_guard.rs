//! Best-effort guardrails for leaked Swift STT Python sidecar processes.
//!
//! The normal owner is `swift_stt_engine::shutdown`. This module covers dev
//! restarts, force-quits, and crashes that leave `server.py` orphaned (PPID=1).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sysinfo::{Pid, System};
use tracing::{info, warn};

pub const SIDECAR_CMD_MARKER: &str = "swift-stt-sidecar/server.py";

fn data_base() -> PathBuf {
    dirs::data_local_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join(".local/share")))
        .unwrap_or_else(|| std::env::temp_dir())
}

pub fn pid_file() -> PathBuf {
    data_base().join("AirNote").join("swift-stt-sidecar.pid")
}

pub fn reap_previous() {
    let sys = System::new_all();

    if let Some(pid) = read_pid_file(&pid_file()) {
        if process_is_swift_sidecar_pid(&sys, pid) {
            info!("[swift-guard] reaping previous sidecar from pid file pid={pid}");
            terminate_pid(pid, Duration::from_secs(1));
        } else {
            info!("[swift-guard] ignoring stale swift sidecar pid file pid={pid}");
        }
        let _ = std::fs::remove_file(pid_file());
    }

    for (pid, process) in sys.processes() {
        if process_is_swift_sidecar(process) {
            let raw_pid = pid.as_u32();
            info!(
                "[swift-guard] reaping orphan swift sidecar pid={raw_pid} cmd={}",
                process_cmdline(process)
            );
            terminate_pid(raw_pid, Duration::from_secs(1));
        }
    }
}

pub fn write_pid_file(pid: u32) {
    let path = pid_file();
    if let Some(parent) = path.parent() {
        if let Err(err) = std::fs::create_dir_all(parent) {
            warn!("[swift-guard] failed to create pid dir {parent:?}: {err}");
            return;
        }
    }
    if let Err(err) = std::fs::write(&path, pid.to_string()) {
        warn!("[swift-guard] failed to write pid file {path:?}: {err}");
    }
}

pub fn clear_pid_file() {
    let path = pid_file();
    if let Err(err) = std::fs::remove_file(&path) {
        if err.kind() != std::io::ErrorKind::NotFound {
            warn!("[swift-guard] failed to clear pid file {path:?}: {err}");
        }
    }
}

pub fn kill_from_pid_file() {
    let sys = System::new_all();
    let Some(pid) = read_pid_file(&pid_file()) else {
        return;
    };
    if process_is_swift_sidecar_pid(&sys, pid) {
        warn!("[swift-guard] signal/panic cleanup killing swift sidecar pid={pid}");
        terminate_pid(pid, Duration::from_secs(1));
    }
    clear_pid_file();
}

fn read_pid_file(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn process_cmdline(process: &sysinfo::Process) -> String {
    process
        .cmd()
        .iter()
        .map(|arg| arg.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ")
}

fn process_is_swift_sidecar(process: &sysinfo::Process) -> bool {
    process_cmdline(process).contains(SIDECAR_CMD_MARKER)
}

fn process_is_swift_sidecar_pid(sys: &System, pid: u32) -> bool {
    sys.process(Pid::from_u32(pid))
        .is_some_and(process_is_swift_sidecar)
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

    #[cfg(windows)]
    {
        let _ = (pid, graceful_for);
    }
}

#[cfg(unix)]
fn pid_is_alive(pid: libc::pid_t) -> bool {
    let rc = unsafe { libc::kill(pid, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_matches_sidecar_command_line() {
        let cmd = "/usr/bin/python3 /app/resources/swift-stt-sidecar/server.py --model-dir /tmp";
        assert!(cmd.contains(SIDECAR_CMD_MARKER));
    }
}
