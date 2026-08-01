//! Integrity-first storage for catalog-backed local speech models.

use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use reqwest::Response;
use reqwest::header::{CONTENT_RANGE, RANGE};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter};
use tokio::sync::Notify;

use crate::local_model_catalog::{self, LocalModelDescriptor};

pub const DOWNLOAD_EVENT: &str = "local-model-download";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const STALL_TIMEOUT: Duration = Duration::from_secs(60);
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Serialize)]
pub struct DownloadProgress {
    pub model: String,
    pub name: String,
    pub received: u64,
    pub total: u64,
    pub status: String,
    pub error: Option<String>,
}

struct DownloadControl {
    cancelled: AtomicBool,
    notify: Notify,
}

static ACTIVE_DOWNLOADS: OnceLock<Mutex<HashMap<String, Arc<DownloadControl>>>> = OnceLock::new();
static VERIFIED_MODELS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn active_downloads() -> &'static Mutex<HashMap<String, Arc<DownloadControl>>> {
    ACTIVE_DOWNLOADS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn verified_models() -> &'static Mutex<HashSet<String>> {
    VERIFIED_MODELS.get_or_init(|| Mutex::new(HashSet::new()))
}

struct DownloadGuard {
    key: String,
}

impl Drop for DownloadGuard {
    fn drop(&mut self) {
        if let Ok(mut downloads) = active_downloads().lock() {
            downloads.remove(&self.key);
        }
    }
}

pub fn model_path(model: &LocalModelDescriptor) -> PathBuf {
    said_core::paths::data_dir()
        .join("models")
        .join(model.filename)
}

fn partial_path(model: &LocalModelDescriptor) -> PathBuf {
    model_path(model).with_extension("gguf.part")
}

pub fn installed(model: &LocalModelDescriptor) -> bool {
    fs::metadata(model_path(model))
        .map(|metadata| metadata.is_file() && metadata.len() == model.size_bytes)
        .unwrap_or(false)
}

pub fn installed_size(model: &LocalModelDescriptor) -> u64 {
    fs::metadata(model_path(model))
        .map(|metadata| metadata.len())
        .unwrap_or(0)
}

pub fn ensure_verified(model: &LocalModelDescriptor) -> Result<PathBuf, String> {
    let path = model_path(model);
    if !installed(model) {
        return Err(format!(
            "{} is not installed. Download it in Settings → Speech recognition.",
            model.name
        ));
    }
    if verified_models()
        .lock()
        .map_err(|_| "Local model verification cache is unavailable".to_string())?
        .contains(model.key)
    {
        return Ok(path);
    }
    if let Err(error) = verify_file(&path, model) {
        let _ = fs::remove_file(&path);
        if let Ok(mut verified) = verified_models().lock() {
            verified.remove(model.key);
        }
        return Err(format!(
            "{error} The corrupt copy was removed; download it again."
        ));
    }
    verified_models()
        .lock()
        .map_err(|_| "Local model verification cache is unavailable".to_string())?
        .insert(model.key.to_string());
    Ok(path)
}

fn verify_file(path: &Path, model: &LocalModelDescriptor) -> Result<(), String> {
    let metadata =
        fs::metadata(path).map_err(|error| format!("Couldn't inspect {}: {error}", model.name))?;
    if !metadata.is_file() || metadata.len() != model.size_bytes {
        return Err(format!(
            "{} has the wrong size ({}/{} bytes). Download it again.",
            model.name,
            metadata.len(),
            model.size_bytes
        ));
    }
    let mut file = fs::File::open(path)
        .map_err(|error| format!("Couldn't open {} for verification: {error}", model.name))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 256 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| format!("Couldn't verify {}: {error}", model.name))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    let actual = format!("{:x}", hasher.finalize());
    if actual != model.sha256 {
        return Err(format!(
            "{} failed its integrity check. Delete it and download it again.",
            model.name
        ));
    }
    Ok(())
}

pub fn remove(model: &LocalModelDescriptor) -> Result<u64, String> {
    cancel(model.key)?;
    let path = model_path(model);
    let size = installed_size(model);
    if path.exists() {
        fs::remove_file(&path)
            .map_err(|error| format!("Couldn't delete {}: {error}", model.name))?;
    }
    let _ = fs::remove_file(partial_path(model));
    if let Ok(mut verified) = verified_models().lock() {
        verified.remove(model.key);
    }
    Ok(size)
}

#[tauri::command]
pub async fn download_local_model(app: AppHandle, model: String) -> Result<(), String> {
    let descriptor = local_model_catalog::find(&model)
        .ok_or_else(|| format!("Unknown local speech model: {model}"))?;
    if installed(descriptor) {
        return tauri::async_runtime::spawn_blocking(move || {
            ensure_verified(descriptor).map(|_| ())
        })
        .await
        .map_err(|error| format!("Local model verification task failed: {error}"))?;
    }
    download_model(&app, descriptor).await
}

#[tauri::command]
pub fn cancel_local_model_download(model: String) -> Result<(), String> {
    cancel(&model)
}

fn cancel(model: &str) -> Result<(), String> {
    let downloads = active_downloads()
        .lock()
        .map_err(|_| "Local model download registry is unavailable".to_string())?;
    if let Some(cancelled) = downloads.get(local_model_catalog::canonical_key(model)) {
        cancelled.cancelled.store(true, Ordering::Release);
        cancelled.notify.notify_one();
    }
    Ok(())
}

fn emit(
    app: &AppHandle,
    model: &LocalModelDescriptor,
    received: u64,
    status: &str,
    error: Option<String>,
) {
    let _ = app.emit(
        DOWNLOAD_EVENT,
        DownloadProgress {
            model: model.key.to_string(),
            name: model.filename.to_string(),
            received,
            total: model.size_bytes,
            status: status.to_string(),
            error,
        },
    );
}

fn register_download(
    model: &LocalModelDescriptor,
) -> Result<(Arc<DownloadControl>, DownloadGuard), String> {
    let mut downloads = active_downloads()
        .lock()
        .map_err(|_| "Local model download registry is unavailable".to_string())?;
    if downloads.contains_key(model.key) {
        return Err(format!("{} is already downloading.", model.name));
    }
    let cancelled = Arc::new(DownloadControl {
        cancelled: AtomicBool::new(false),
        notify: Notify::new(),
    });
    downloads.insert(model.key.to_string(), Arc::clone(&cancelled));
    Ok((
        cancelled,
        DownloadGuard {
            key: model.key.to_string(),
        },
    ))
}

async fn download_model(
    app: &AppHandle,
    model: &'static LocalModelDescriptor,
) -> Result<(), String> {
    let (cancelled, _guard) = register_download(model)?;
    let sources = [
        ("huggingface", model.download_url(), 3_u64),
        ("mirror", model.mirror_url(), 2_u64),
    ];
    let mut last_error = None;
    for (source, url, attempts) in sources {
        for attempt in 1..=attempts {
            if cancelled.cancelled.load(Ordering::Acquire) {
                emit(app, model, installed_size(model), "cancelled", None);
                return Err(format!("{} download cancelled", model.name));
            }
            match download_attempt(app, model, &cancelled, &url).await {
                Ok(()) => return Ok(()),
                Err(error) if cancelled.cancelled.load(Ordering::Acquire) => return Err(error),
                Err(error) => {
                    last_error = Some(error.clone());
                    let more_attempts = attempt < attempts || source == "huggingface";
                    if !more_attempts {
                        break;
                    }
                    let delay = Duration::from_secs(attempt * 2);
                    tracing::warn!(model = model.key, source, attempt, %error, ?delay, "[local-model] retrying download");
                    emit(
                        app,
                        model,
                        fs::metadata(partial_path(model))
                            .map(|meta| meta.len())
                            .unwrap_or(0),
                        "retrying",
                        Some(error),
                    );
                    tokio::select! {
                        _ = tokio::time::sleep(delay) => {}
                        _ = cancelled.notify.notified() => {
                            emit(app, model, installed_size(model), "cancelled", None);
                            return Err(format!("{} download cancelled", model.name));
                        }
                    }
                }
            }
        }
    }
    Err(last_error.unwrap_or_else(|| format!("{} download failed", model.name)))
}

async fn download_attempt(
    app: &AppHandle,
    model: &'static LocalModelDescriptor,
    control: &DownloadControl,
    url: &str,
) -> Result<(), String> {
    let destination = model_path(model);
    let directory = destination
        .parent()
        .ok_or_else(|| "Local model path has no parent directory".to_string())?;
    fs::create_dir_all(directory)
        .map_err(|error| format!("Couldn't create models folder: {error}"))?;
    let partial = partial_path(model);

    if installed(model) {
        ensure_verified(model)?;
        emit(app, model, model.size_bytes, "done", None);
        return Ok(());
    }
    if destination.exists() {
        fs::remove_file(&destination)
            .map_err(|error| format!("Couldn't replace invalid {}: {error}", model.name))?;
    }

    let mut offset = fs::metadata(&partial).map(|meta| meta.len()).unwrap_or(0);
    if offset > model.size_bytes {
        fs::remove_file(&partial)
            .map_err(|error| format!("Couldn't discard oversized partial model: {error}"))?;
        offset = 0;
    }
    if offset == model.size_bytes {
        return verify_and_activate_async(app, model, partial, destination).await;
    }
    ensure_free_disk(directory, model.size_bytes.saturating_sub(offset))?;

    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        .map_err(|error| format!("Local model HTTP client failed: {error}"))?;
    let mut request = client.get(url);
    if offset > 0 {
        request = request.header(RANGE, format!("bytes={offset}-"));
    }
    let response = tokio::select! {
        response = request.send() => response,
        _ = control.notify.notified() => {
            emit(app, model, offset, "cancelled", None);
            return Err(format!("{} download cancelled", model.name));
        }
    };
    let mut response = response.map_err(|error| {
        let message = format!("{} download request failed: {error}", model.name);
        emit(app, model, offset, "error", Some(message.clone()));
        message
    })?;

    if offset > 0 && response.status() == reqwest::StatusCode::OK {
        // The origin ignored Range. Restart safely instead of appending a full
        // response to the partial file.
        offset = 0;
    } else if offset > 0 && response.status() == reqwest::StatusCode::PARTIAL_CONTENT {
        validate_content_range(&response, offset).map_err(|message| {
            emit(app, model, offset, "error", Some(message.clone()));
            message
        })?;
    } else if !response.status().is_success() {
        let message = format!("{} download failed: HTTP {}", model.name, response.status());
        emit(app, model, offset, "error", Some(message.clone()));
        return Err(message);
    }

    let remaining = model.size_bytes.saturating_sub(offset);
    if let Some(length) = response.content_length()
        && length > remaining
    {
        let message = format!(
            "{} download advertised too many bytes ({length} remaining, expected at most {remaining}).",
            model.name
        );
        emit(app, model, offset, "error", Some(message.clone()));
        return Err(message);
    }

    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(offset == 0)
        .open(&partial)
        .map_err(|error| format!("Couldn't open {} partial download: {error}", model.name))?;
    if offset > 0 {
        file.seek(SeekFrom::Start(offset))
            .map_err(|error| format!("Couldn't resume {}: {error}", model.name))?;
    }

    emit(app, model, offset, "downloading", None);
    let mut received = offset;
    let mut last_emit = Instant::now();
    loop {
        if control.cancelled.load(Ordering::Acquire) {
            file.flush().ok();
            emit(app, model, received, "cancelled", None);
            return Err(format!("{} download cancelled", model.name));
        }
        let next_chunk = tokio::select! {
            chunk = tokio::time::timeout(STALL_TIMEOUT, response.chunk()) => chunk,
            _ = control.notify.notified() => {
                file.flush().ok();
                emit(app, model, received, "cancelled", None);
                return Err(format!("{} download cancelled", model.name));
            }
        };
        let chunk = match next_chunk {
            Ok(Ok(chunk)) => chunk,
            Ok(Err(error)) => {
                let message = format!("{} download failed: {error}", model.name);
                emit(app, model, received, "error", Some(message.clone()));
                return Err(message);
            }
            Err(_) => {
                let message = format!(
                    "{} download made no progress for {} seconds. It can be resumed.",
                    model.name,
                    STALL_TIMEOUT.as_secs()
                );
                emit(app, model, received, "error", Some(message.clone()));
                return Err(message);
            }
        };
        let Some(chunk) = chunk else {
            break;
        };
        let next = received.saturating_add(chunk.len() as u64);
        if next > model.size_bytes {
            let message = format!("{} download exceeded its expected size.", model.name);
            emit(app, model, received, "error", Some(message.clone()));
            return Err(message);
        }
        file.write_all(&chunk).map_err(|error| {
            let message = format!("Couldn't write {} download: {error}", model.name);
            emit(app, model, received, "error", Some(message.clone()));
            message
        })?;
        received = next;
        if last_emit.elapsed() >= PROGRESS_INTERVAL {
            emit(app, model, received, "downloading", None);
            last_emit = Instant::now();
        }
    }
    file.flush()
        .map_err(|error| format!("Couldn't flush {} download: {error}", model.name))?;
    file.sync_all().ok();
    drop(file);

    if received != model.size_bytes {
        let message = format!(
            "{} download ended early ({received}/{} bytes). It can be resumed.",
            model.name, model.size_bytes
        );
        emit(app, model, received, "error", Some(message.clone()));
        return Err(message);
    }
    verify_and_activate_async(app, model, partial, destination).await
}

fn ensure_free_disk(directory: &Path, required_bytes: u64) -> Result<(), String> {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let available = disks
        .iter()
        .filter(|disk| directory.starts_with(disk.mount_point()))
        .max_by_key(|disk| disk.mount_point().as_os_str().len())
        .map(sysinfo::Disk::available_space);
    let reserve = 100 * 1024 * 1024;
    if let Some(available) = available
        && available < required_bytes.saturating_add(reserve)
    {
        return Err(format!(
            "Not enough disk space for this model. Free at least {} MB and try again.",
            required_bytes.saturating_add(reserve).div_ceil(1_000_000)
        ));
    }
    Ok(())
}

fn validate_content_range(response: &Response, expected_start: u64) -> Result<(), String> {
    let value = response
        .headers()
        .get(CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| "Resume response is missing Content-Range".to_string())?;
    let start = content_range_start(value)
        .ok_or_else(|| format!("Resume response has an invalid Content-Range: {value}"))?;
    if start != expected_start {
        return Err(format!(
            "Resume response started at byte {start}, expected {expected_start}."
        ));
    }
    Ok(())
}

fn content_range_start(value: &str) -> Option<u64> {
    value
        .strip_prefix("bytes ")
        .and_then(|value| value.split_once('-'))
        .and_then(|(start, _)| start.parse::<u64>().ok())
}

async fn verify_and_activate_async(
    app: &AppHandle,
    model: &'static LocalModelDescriptor,
    partial: PathBuf,
    destination: PathBuf,
) -> Result<(), String> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        verify_and_activate(&app, model, &partial, &destination)
    })
    .await
    .map_err(|error| format!("Local model verification task failed: {error}"))?
}

fn verify_and_activate(
    app: &AppHandle,
    model: &LocalModelDescriptor,
    partial: &Path,
    destination: &Path,
) -> Result<(), String> {
    emit(app, model, model.size_bytes, "verifying", None);
    if let Err(message) = verify_file(partial, model) {
        let _ = fs::remove_file(partial);
        emit(app, model, model.size_bytes, "error", Some(message.clone()));
        return Err(message);
    }
    fs::rename(partial, destination).map_err(|error| {
        let message = format!("Couldn't activate {}: {error}", model.name);
        emit(app, model, model.size_bytes, "error", Some(message.clone()));
        message
    })?;
    verified_models()
        .lock()
        .map_err(|_| "Local model verification cache is unavailable".to_string())?
        .insert(model.key.to_string());
    emit(app, model, model.size_bytes, "done", None);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::content_range_start;

    #[test]
    fn parses_only_valid_content_range_starts() {
        assert_eq!(content_range_start("bytes 42-99/100"), Some(42));
        assert_eq!(content_range_start("bytes */100"), None);
        assert_eq!(content_range_start("items 42-99/100"), None);
    }
}
