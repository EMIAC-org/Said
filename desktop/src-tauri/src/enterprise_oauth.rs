//! Ephemeral localhost HTTP listener for enterprise Lark OAuth callbacks.
//!
//! Deep links (`airnote://auth/callback`) do not fire reliably in `tauri dev` on
//! macOS, so the desktop app binds `127.0.0.1:0` and passes the port to the
//! control-plane. After Lark sign-in, the server callback page redirects the
//! browser to `http://127.0.0.1:{port}/callback?token=…`.

use std::sync::{Mutex, OnceLock};

use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

const LISTENER_TIMEOUT_SECS: u64 = 300;

struct ListenerSlot {
    shutdown: Option<oneshot::Sender<()>>,
}

static LISTENER: OnceLock<Mutex<ListenerSlot>> = OnceLock::new();

fn slot() -> &'static Mutex<ListenerSlot> {
    LISTENER.get_or_init(|| Mutex::new(ListenerSlot { shutdown: None }))
}

pub fn stop_listener() {
    let mut guard = slot().lock().expect("enterprise oauth listener lock");
    if let Some(tx) = guard.shutdown.take() {
        let _ = tx.send(());
    }
}

pub fn emit_token(app: &AppHandle, token: &str) {
    let _ = app.emit(
        "enterprise-oauth-token",
        serde_json::json!({ "token": token }),
    );
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn parse_token_from_request(request: &str) -> Option<String> {
    let first_line = request.lines().next()?;
    let path = first_line.split_whitespace().nth(1)?;
    if !path.starts_with("/callback") {
        return None;
    }
    let query = path.split('?').nth(1)?;
    for part in query.split('&') {
        if let Some(token) = part.strip_prefix("token=") {
            let token = token.trim();
            if !token.is_empty() {
                return Some(token.to_string());
            }
        }
    }
    None
}

async fn write_response(stream: &mut tokio::net::TcpStream, body: &str) -> std::io::Result<()> {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await
}

const SUCCESS_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>AirNote — Connected</title>
<style>
  *{margin:0;padding:0;box-sizing:border-box}
  body{font-family:Inter,system-ui,sans-serif;background:hsl(240 6% 6%);color:hsl(240 6% 92%);min-height:100vh;display:flex;align-items:center;justify-content:center}
  .card{background:hsl(240 5% 10%);border:1px solid hsl(240 5% 18%);border-radius:20px;padding:36px;max-width:380px;width:calc(100vw - 32px);text-align:center}
  .icon{width:44px;height:44px;border-radius:14px;background:hsl(226 80% 78% / 0.14);display:flex;align-items:center;justify-content:center;margin:0 auto 16px;color:hsl(226 80% 78%)}
  h1{font-size:18px;font-weight:600;margin-bottom:8px}
  p{font-size:13px;color:hsl(240 4% 58%);line-height:1.5}
</style>
</head>
<body>
<div class="card">
  <div class="icon">✓</div>
  <h1>Connected to AirNote</h1>
  <p>You can close this tab and return to the app.</p>
</div>
</body>
</html>"#;

pub async fn start_listener(app: AppHandle) -> Result<u16, String> {
    stop_listener();

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| format!("could not bind localhost callback port: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("local_addr: {e}"))?
        .port();

    let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
    {
        let mut guard = slot().lock().expect("enterprise oauth listener lock");
        guard.shutdown = Some(shutdown_tx);
    }

    tracing::info!("[enterprise-oauth] listening on 127.0.0.1:{port}/callback");

    tokio::spawn(async move {
        let deadline =
            tokio::time::Instant::now() + std::time::Duration::from_secs(LISTENER_TIMEOUT_SECS);

        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                tracing::info!("[enterprise-oauth] listener timed out");
                break;
            }

            tokio::select! {
                _ = &mut shutdown_rx => {
                    tracing::info!("[enterprise-oauth] listener stopped");
                    break;
                }
                accept = tokio::time::timeout(remaining, listener.accept()) => {
                    match accept {
                        Ok(Ok((mut stream, _))) => {
                            let mut buf = vec![0u8; 8192];
                            let n = stream.read(&mut buf).await.unwrap_or(0);
                            let request = String::from_utf8_lossy(&buf[..n]);
                            if let Some(token) = parse_token_from_request(&request) {
                                tracing::info!("[enterprise-oauth] received token via localhost callback");
                                emit_token(&app, &token);
                                let _ = write_response(&mut stream, SUCCESS_HTML).await;
                                break;
                            }
                            let _ = write_response(&mut stream, SUCCESS_HTML).await;
                        }
                        Ok(Err(e)) => {
                            tracing::warn!("[enterprise-oauth] accept failed: {e}");
                        }
                        Err(_) => {
                            tracing::info!("[enterprise-oauth] listener timed out");
                            break;
                        }
                    }
                }
            }
        }

        stop_listener();
    });

    Ok(port)
}
