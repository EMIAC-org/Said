//! Hugging Face download + status for Oriserve Swift local STT weights.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use reqwest::StatusCode;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

const MODEL_DOWNLOAD_EVENT: &str = "swift-model-download";
const HF_BASE: &str = "https://huggingface.co/Oriserve/Whisper-Hindi2Hinglish-Swift/resolve/main";
const DOWNLOAD_ATTEMPTS: usize = 4;

/// Tokenizer + config first; weights last so early failures are cheap.
const MODEL_FILES: &[(&str, u64)] = &[
    ("config.json", 1_193),
    ("preprocessor_config.json", 339),
    ("generation_config.json", 3_562),
    ("special_tokens_map.json", 2_025),
    ("added_tokens.json", 34_628),
    ("tokenizer_config.json", 282_615),
    ("vocab.json", 1_036_558),
    ("merges.txt", 493_869),
    ("tokenizer.json", 3_930_462),
    ("model.safetensors", 290_403_936),
];

#[derive(Debug, Clone, Serialize)]
pub struct SwiftModelDownloadProgress {
    pub received: u64,
    pub total: u64,
    pub status: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SwiftModelStatus {
    pub installed: bool,
    pub size_bytes: u64,
    pub path: String,
    pub downloading_percent: Option<u8>,
}

fn downloads_inflight() -> &'static Mutex<HashSet<()>> {
    static CELL: OnceLock<Mutex<HashSet<()>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(HashSet::new()))
}

fn download_cancelled() -> &'static Mutex<bool> {
    static CELL: OnceLock<Mutex<bool>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(false))
}

pub fn model_dir() -> PathBuf {
    said_core::paths::swift_model_dir()
}

pub fn is_installed() -> bool {
    MODEL_FILES
        .iter()
        .all(|(name, size_hint)| is_model_file_complete(&model_dir().join(name), *size_hint))
}

fn is_model_file_complete(path: &Path, size_hint: u64) -> bool {
    let Ok(meta) = path.metadata() else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    let min_size = size_hint.saturating_sub(size_hint / 20).max(1);
    meta.len() >= min_size
}

pub fn installed_size_bytes() -> u64 {
    let dir = model_dir();
    if !dir.is_dir() {
        return 0;
    }
    fs::read_dir(&dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter_map(|e| e.metadata().ok())
        .filter(|m| m.is_file())
        .map(|m| m.len())
        .sum()
}

pub fn model_status() -> SwiftModelStatus {
    SwiftModelStatus {
        installed: is_installed(),
        size_bytes: installed_size_bytes(),
        path: model_dir().display().to_string(),
        downloading_percent: None,
    }
}

#[tauri::command]
pub fn swift_stt_model_status() -> SwiftModelStatus {
    model_status()
}

#[tauri::command]
pub async fn swift_stt_download_model(app: AppHandle) -> Result<(), String> {
    if is_installed() {
        return Ok(());
    }
    {
        let mut inflight = downloads_inflight()
            .lock()
            .map_err(|_| "download registry poisoned".to_string())?;
        if !inflight.insert(()) {
            return Err("Swift model is already downloading".to_string());
        }
    }
    if let Ok(mut cancelled) = download_cancelled().lock() {
        *cancelled = false;
    }

    let result = tauri::async_runtime::spawn_blocking(move || download_all_blocking(&app)).await;

    if let Ok(mut inflight) = downloads_inflight().lock() {
        inflight.remove(&());
    }

    result.map_err(|e| format!("download task failed: {e}"))?
}

#[tauri::command]
pub fn swift_stt_cancel_download() -> Result<(), String> {
    if let Ok(mut cancelled) = download_cancelled().lock() {
        *cancelled = true;
    }
    Ok(())
}

#[tauri::command]
pub fn swift_stt_delete_model() -> Result<(), String> {
    let _ = swift_stt_cancel_download();
    // The Swift STT engine only exists on macOS; on other targets there's
    // nothing running to shut down before we remove the weights.
    #[cfg(target_os = "macos")]
    crate::swift_stt_engine::shutdown();
    remove_model_dir(&model_dir())
}

fn remove_model_dir(dir: &Path) -> Result<(), String> {
    if !dir.exists() {
        return Ok(());
    }
    fs::remove_dir_all(dir).map_err(|e| format!("delete failed: {e}"))
}

fn total_hint_bytes() -> u64 {
    MODEL_FILES.iter().map(|(_, size)| size).sum()
}

fn emit(app: &AppHandle, received: u64, total: u64, status: &str, error: Option<String>) {
    let _ = app.emit(
        MODEL_DOWNLOAD_EVENT,
        SwiftModelDownloadProgress {
            received,
            total,
            status: status.to_string(),
            error,
        },
    );
}

fn http_client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .user_agent("AirNote/2.4.1 (+https://airnote.emiactech.com; swift-stt-model-download)")
        .redirect(reqwest::redirect::Policy::limited(10))
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|e| format!("http client: {e}"))
}

fn download_all_blocking(app: &AppHandle) -> Result<(), String> {
    let dir = model_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("couldn't create models folder: {e}"))?;

    let client = http_client()?;
    let total = total_hint_bytes();
    let mut received_base: u64 = 0;
    emit(app, 0, total, "downloading", None);

    for (name, size_hint) in MODEL_FILES {
        if is_cancelled() {
            emit(app, received_base, total, "cancelled", None);
            return Err("cancelled".to_string());
        }
        let dest = dir.join(name);
        if is_model_file_complete(&dest, *size_hint) {
            received_base += dest.metadata().map(|m| m.len()).unwrap_or(*size_hint);
            emit(app, received_base.min(total), total, "downloading", None);
            continue;
        }
        if dest.exists() {
            let _ = fs::remove_file(&dest);
        }
        let url = format!("{HF_BASE}/{name}");
        let file_received = download_file_blocking(
            app,
            &client,
            &url,
            name,
            &dest,
            *size_hint,
            received_base,
            total,
        )?;
        received_base += file_received;
    }

    if !is_installed() {
        let msg = "download finished but model.safetensors is missing".to_string();
        emit(app, received_base, total, "error", Some(msg.clone()));
        return Err(msg);
    }

    // Remove any leftover partial files after a successful install.
    for (name, _) in MODEL_FILES {
        let part = dir.join(name).with_extension("part");
        let _ = fs::remove_file(part);
    }

    emit(app, total, total, "done", None);
    Ok(())
}

fn is_cancelled() -> bool {
    download_cancelled().lock().map(|g| *g).unwrap_or(false)
}

fn download_file_blocking(
    app: &AppHandle,
    client: &reqwest::blocking::Client,
    url: &str,
    name: &str,
    dest: &Path,
    size_hint: u64,
    received_base: u64,
    total: u64,
) -> Result<u64, String> {
    let part = dest.with_extension("part");
    let mut last_err = String::new();
    for attempt in 1..=DOWNLOAD_ATTEMPTS {
        if is_cancelled() {
            let _ = fs::remove_file(&part);
            emit(app, received_base, total, "cancelled", None);
            return Err("cancelled".to_string());
        }
        match download_file_attempt(
            app,
            client,
            url,
            name,
            dest,
            &part,
            size_hint,
            received_base,
            total,
        ) {
            Ok(bytes) => return Ok(bytes),
            Err(e) => {
                last_err = e;
                if attempt < DOWNLOAD_ATTEMPTS {
                    tracing::warn!(
                        "[swift_model] {name} attempt {attempt}/{DOWNLOAD_ATTEMPTS} failed: {last_err}"
                    );
                    std::thread::sleep(Duration::from_secs(attempt as u64 * 2));
                }
            }
        }
    }
    emit(app, received_base, total, "error", Some(last_err.clone()));
    Err(last_err)
}

fn download_file_attempt(
    app: &AppHandle,
    client: &reqwest::blocking::Client,
    url: &str,
    name: &str,
    dest: &Path,
    part: &Path,
    size_hint: u64,
    received_base: u64,
    total: u64,
) -> Result<u64, String> {
    let mut resume_from = if part.is_file() {
        part.metadata().map(|m| m.len()).unwrap_or(0)
    } else {
        0
    };

    let mut request = client.get(url);
    if resume_from > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={resume_from}-"));
    }

    let mut response = request
        .send()
        .map_err(|e| format!("request failed for {name}: {e}"))?;
    let status = response.status();

    if status == StatusCode::RANGE_NOT_SATISFIABLE {
        let _ = fs::remove_file(part);
        resume_from = 0;
        response = client
            .get(url)
            .send()
            .map_err(|e| format!("request failed for {name}: {e}"))?;
        if !response.status().is_success() {
            return Err(format!(
                "download failed for {name}: HTTP {}",
                response.status()
            ));
        }
    } else if !status.is_success() {
        let _ = fs::remove_file(part);
        return Err(format!("download failed for {name}: HTTP {status}"));
    }

    let file_total = if status == StatusCode::PARTIAL_CONTENT {
        response
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split('/').nth(1))
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(size_hint)
    } else {
        resume_from = 0;
        response.content_length().unwrap_or(size_hint)
    };

    let mut file = if resume_from > 0 && status == StatusCode::PARTIAL_CONTENT {
        OpenOptions::new()
            .append(true)
            .open(part)
            .map_err(|e| format!("open resume for {name}: {e}"))?
    } else {
        if part.is_file() {
            let _ = fs::remove_file(part);
        }
        File::create(part).map_err(|e| format!("create temp for {name}: {e}"))?
    };

    let mut buf = vec![0u8; 256 * 1024];
    let mut received = resume_from;
    let mut last_emit = 0u64;
    emit(app, received_base + received, total, "downloading", None);

    loop {
        if is_cancelled() {
            drop(file);
            emit(
                app,
                (received_base + received).min(total),
                total,
                "cancelled",
                None,
            );
            return Err("cancelled".to_string());
        }
        let n = response
            .read(&mut buf)
            .map_err(|e| format!("read failed for {name}: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])
            .map_err(|e| format!("write failed for {name}: {e}"))?;
        received += n as u64;
        if received - last_emit >= 256 * 1024 || received >= file_total {
            last_emit = received;
            emit(
                app,
                (received_base + received).min(total),
                total,
                "downloading",
                None,
            );
        }
    }
    drop(file);

    if file_total > 0 && received < file_total.saturating_mul(9) / 10 {
        return Err(format!(
            "download incomplete for {name}: got {received} of {file_total} bytes"
        ));
    }

    fs::rename(part, dest).map_err(|e| format!("finalize {name}: {e}"))?;
    Ok(dest.metadata().map(|m| m.len()).unwrap_or(file_total))
}
