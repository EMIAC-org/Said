//! Native Swift notch-HUD sidecar bridge (append-only, env-flag-gated).
//!
//! Spawns the `airnote-notch` Swift binary and speaks newline-delimited JSON:
//!   * Rust → sidecar (stdin):  status events mirrored from the app
//!   * sidecar → Rust (stdout): user actions (confirm / skip / retry / …)
//!
//! Logs from the sidecar go to its stderr (inherited). The sidecar
//! self-terminates when its stdin closes (i.e. when Tauri exits and the pipe
//! drops), so no explicit kill is required.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};

use tracing::{info, warn};

/// Cheap-to-clone handle to the running sidecar. Clones share one stdin pipe.
#[derive(Clone)]
pub struct NotchSidecar {
    inner: Arc<Inner>,
}

struct Inner {
    stdin: Mutex<ChildStdin>,
    // Held only to keep the child owned for the app's lifetime; dropping it
    // closes the pipe and the sidecar exits on its own.
    _child: Mutex<Child>,
}

impl NotchSidecar {
    /// Send one message to the sidecar. Best-effort — a dead pipe is ignored.
    pub fn send(&self, msg: &serde_json::Value) {
        if let Ok(mut w) = self.inner.stdin.lock() {
            let line = msg.to_string();
            let _ = w.write_all(line.as_bytes());
            let _ = w.write_all(b"\n");
            let _ = w.flush();
        }
    }
}

/// Spawn the sidecar if its binary can be located. `on_action` runs on a
/// dedicated reader thread for each JSON line the sidecar emits.
pub fn spawn<F>(on_action: F) -> Option<NotchSidecar>
where
    F: Fn(serde_json::Value) + Send + 'static,
{
    let bin = find_binary()?;
    info!("[notch] spawning sidecar {bin:?}");

    let mut child = match Command::new(&bin)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            warn!("[notch] spawn failed: {e}");
            return None;
        }
    };

    let stdin = child.stdin.take()?;
    let stdout = child.stdout.take()?;

    std::thread::Builder::new()
        .name("notch-reader".into())
        .spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                match serde_json::from_str::<serde_json::Value>(trimmed) {
                    Ok(v) => on_action(v),
                    Err(e) => warn!("[notch] bad action line: {e}"),
                }
            }
            info!("[notch] sidecar stdout closed");
        })
        .ok()?;

    Some(NotchSidecar {
        inner: Arc::new(Inner {
            stdin: Mutex::new(stdin),
            _child: Mutex::new(child),
        }),
    })
}

#[cfg(windows)]
const NOTCH_BIN: &str = "airnote-notch.exe";
#[cfg(not(windows))]
const NOTCH_BIN: &str = "airnote-notch";

/// Locate the sidecar binary. Bundled apps ship it as a sibling of the main
/// exe (Tauri `externalBin` strips the target-triple suffix); dev builds find
/// the raw `swift build` output or the copied `binaries/` artifact.
fn find_binary() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join(NOTCH_BIN)); // bundled sibling
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        // `cargo tauri dev` runs from desktop/src-tauri
        candidates.push(cwd.join("../notch-sidecar/.build/release/AirNoteNotch"));
        candidates.push(cwd.join("notch-sidecar/.build/release/AirNoteNotch"));
        candidates.push(cwd.join("binaries").join(NOTCH_BIN));
        candidates.push(cwd.join("src-tauri").join("binaries").join(NOTCH_BIN));
    }

    candidates.into_iter().find(|p| p.exists())
}
