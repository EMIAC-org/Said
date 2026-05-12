//! Cross-platform types shared between the macOS and Windows implementations.
//!
//! [`AxDiagnostics`] is the serializable result of `diagnose_focused_field()`.
//! It's the same shape on both platforms so the Tauri `/diagnose-ax` route can
//! deserialize uniformly. On Windows the `ax_trusted` field is always `true`
//! (UIA requires no permission) and `methods` carries UIA-strategy results
//! instead of AX strategy results — the wire shape is identical.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct AxMethodResult {
    pub method: String,
    pub label: String,
    pub ok: bool,
    pub text: Option<String>,
    pub err: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AxDiagnostics {
    pub ax_trusted: bool,
    pub app_name: Option<String>,
    pub app_pid: Option<i32>,
    pub element_role: Option<String>,
    pub attributes: Vec<String>,
    pub methods: Vec<AxMethodResult>,
    pub clipboard: String,
}
