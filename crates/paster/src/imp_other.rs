//! Stub paster impl for platforms that are neither macOS nor Windows.
//! Linux dev boxes / future targets compile cleanly; nothing actually
//! injects keystrokes.

pub fn request_permission() {}
pub fn request_input_monitoring() {}

pub fn is_accessibility_granted() -> bool {
    false
}

pub fn read_focused_value_fast() -> Option<String> {
    None
}
pub fn read_focused_value_first() -> Option<String> {
    None
}
pub fn read_focused_value() -> Option<String> {
    None
}
pub fn read_focused_value_fast_for_pid(_pid: i32) -> Option<String> {
    None
}
pub fn read_focused_value_first_for_pid(_pid: i32) -> Option<String> {
    None
}
pub fn capture_focused_text_via_selection() -> Option<String> {
    None
}
pub fn read_selected_text() -> Option<String> {
    None
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AxMethodResult {
    pub method: String,
    pub label: String,
    pub ok: bool,
    pub text: Option<String>,
    pub err: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AxDiagnostics {
    pub ax_trusted: bool,
    pub app_name: Option<String>,
    pub app_pid: Option<i32>,
    pub element_role: Option<String>,
    pub attributes: Vec<String>,
    pub methods: Vec<AxMethodResult>,
    pub clipboard: String,
}

pub fn diagnose_focused_field() -> AxDiagnostics {
    AxDiagnostics {
        ax_trusted: false,
        app_name: None,
        app_pid: None,
        element_role: None,
        attributes: vec![],
        methods: vec![],
        clipboard: String::new(),
    }
}

pub fn focused_pid() -> Option<i32> {
    None
}
pub fn unlock_focused_app_now() -> Option<i32> {
    None
}
pub fn lock_frontmost_app_now() -> Option<i32> {
    None
}

pub fn type_text(_text: &str) -> Result<bool, String> {
    Ok(false)
}

pub fn paste(_text: &str) -> Result<(), String> {
    Err("paste not implemented on this platform".into())
}

pub fn paste_replacing(_text: &str) -> Result<(), String> {
    Err("paste_replacing not implemented on this platform".into())
}
