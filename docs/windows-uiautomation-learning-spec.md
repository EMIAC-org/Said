# Windows UIAutomation — Learning Pipeline Parity Spec

**Status:** IMPLEMENTED (code) — `crates/paster/src/uia.rs` + wired
`imp_windows.rs`. Cross-checks clean for `x86_64-pc-windows-msvc`; macOS host
unaffected. REMAINING: runtime testing on a real Windows box (see Verification).
This doc is the design reference; the executed version lives in the plan file.
**Goal:** make the 30-second edit-watch / correction-learning pipeline work on
Windows identically to macOS by implementing the focused-field reads that are
currently stubbed in `crates/paster/src/imp_windows.rs`.

This is the ONLY remaining work to bring Windows to full learning parity. The
entire backend half (classifier, promotion gates, embeddings, SQLite persistence)
is already platform-agnostic and verified. Typing and pasting already work on
Windows. Today the stubs return `None`, so the watch loop never sees text → never
classifies → learning is silently skipped (no errors, dictation works fine).

---

## 0. Why this is the whole job

The macOS learning flow is:

1. AirNote pastes polished text into the focused field.
2. A background watcher reads the field text repeatedly for up to ~30s.
3. When the text stops changing (user finished editing), it diffs paste-vs-final
   and POSTs to `/v1/classify-edit`.
4. Backend classifies (4-way), runs 3 promotion gates, learns the correction.

The ONLY macOS-specific input is step 2's "read the focused field text." On
Windows that maps to UI Automation (UIA). Implement the reads → learning works.

---

## 1. Functions to implement (exact contracts)

All live in `crates/paster/src/imp_windows.rs`. **Signatures must stay byte-for-byte
identical to the macOS versions in `crates/paster/src/lib.rs`** — the callers in
`desktop/src-tauri/src/main.rs` are already platform-agnostic and must not change.

| Function | Signature | Returns | Contract |
|---|---|---|---|
| `read_focused_value_fast` | `() -> Option<String>` | full field text | **Hot path.** No blocking, no sleep, no a11y "unlock". Safe to call every 30ms. `None` if no value. |
| `read_focused_value_fast_for_pid` | `(pid: i32) -> Option<String>` | full field text | Same, but only if the focused element belongs to `pid` (target-app lock); else `None`. |
| `read_focused_value_first` | `() -> Option<String>` | full field text | **One-shot, thorough.** ValuePattern → (activate a11y) → TextPattern → bounded subtree walk. Slower; used for initial capture + fallback. |
| `read_focused_value_first_for_pid` | `(pid: i32) -> Option<String>` | full field text | Same, PID-locked. |
| `read_selected_text` | `() -> Option<String>` | selected text only | TextPattern `GetSelection` → clipboard Ctrl+C fallback. |
| `capture_focused_text_via_selection` | `() -> Option<String>` | full field text | Last resort for a11y-blind apps: save clipboard → Ctrl+A → Ctrl+C → read → restore clipboard. |
| `focused_pid` | `() -> Option<i32>` | PID | PID of the foreground/focused app. Used for app-switch detection + target lock. |
| `unlock_focused_app_now` | `() -> Option<i32>` | PID | macOS sets AXEnhancedUserInterface/AXManualAccessibility. On Windows = activate Chromium/Electron a11y on the foreground window (WM_GETOBJECT probe). Return foreground PID. Idempotent. |
| `lock_frontmost_app_now` | `() -> Option<i32>` | PID | Capture foreground PID *before* the HUD steals focus, then run the same a11y probe. Return that PID. |
| `diagnose_focused_field` | `() -> AxDiagnostics` | struct | Fill the existing `AxDiagnostics` (in `lib.rs`) with per-method success/failure for the troubleshooting UI. |

`None` semantics everywhere: read failed, field empty, withheld (password), or
permission/UIPI denied. Callers already treat `None` as "skip this tick."

### Caller behavior to preserve (from `desktop/src-tauri/src/main.rs`)
- `start_edit_watcher` → `watch_for_edit`.
- **Initial capture:** sleep 400ms, then up to 3 `read_focused_value_first[_for_pid]`
  attempts at delays 0ms / 300ms / 500ms; first non-`None` becomes `initial_value`
  (else empty string).
- **Poll loop:** 50ms interval for the first 2s (FAST), then 200ms (SLOW). Each tick
  calls `read_focused_value_fast[_for_pid]` wrapped in `blocking_ax_option` (a 500ms
  timeout on a blocking thread).
- **Stabilization / timeouts** (`edit_watch_timeouts`, scales by word count):
  `max_duration` = 15s + 0.6s/word (cap 45s); `idle_timeout` = 6s + 0.15s/word
  (cap 15s); `stable_settle_secs` = 3s + 0.1s/word (cap 10s).
- **App-switch:** each tick calls `focused_pid`; if it changes and there's no target
  lock → exit; if locked → keep watching but the capture is flagged (gates reject it).
- On stabilize → `api::classify_edit(recording_id, current_value, initial_value,
  capture_method, CaptureMeta { time_since_paste_ms, app_switched, matches_clipboard })`.

These timings/retries are the behavioral contract — the Windows reads just need to
return the right strings fast enough; do not change the watcher.

---

## 2. Approach (decided from research, 2026-05-29)

- **Crate:** add `uiautomation` (v0.25.x) — a safe wrapper over `windows-rs`. Avoids
  hand-rolling `VARIANT`/`BSTR`/pattern-cast COM `unsafe` on the hot path. Drop to the
  raw `windows` crate only for the foreground-window/PID/probe bits.
- **Threading:** UIA is COM and MUST run from a **dedicated MTA thread** (Microsoft
  guidance: clients that read other apps' UI use a separate, window-less, MTA thread or
  risk deadlocks/hangs). Initialize COM **once** at thread start
  (`CoInitializeEx(None, COINIT_MULTITHREADED)`), create one `IUIAutomation` + one
  reusable `CacheRequest`, reuse for the thread's life, `CoUninitialize` at shutdown.
- **No COM pointer crosses a thread boundary** (apartment affinity). Only plain Rust
  values (`String`, `i32`) travel over channels.
- **Hang guard:** the public functions send a request to the worker and `recv_timeout`
  on the reply (UIA's own `TransactionTimeout`/`ConnectionTimeout` are unreliable). A
  wedged provider costs one dropped tick, never a frozen daemon.

---

## 3. Module design

New file: `crates/paster/src/uia.rs` (compiled only on Windows). Owns the worker;
`imp_windows.rs` calls into it.

```rust
// crates/paster/src/uia.rs   (#[cfg(target_os = "windows")])
use std::sync::OnceLock;
use std::sync::mpsc::{SyncSender, Receiver, sync_channel};
use std::time::Duration;

enum Req { ValueAny, ValueForPid(i32), Selection, Pid, ActivateForeground }
enum Rep { Text(Option<String>), Pid(Option<i32>) }

struct Reader { tx: SyncSender<(Req, SyncSender<Rep>)> }

static READER: OnceLock<Reader> = OnceLock::new();

fn reader() -> &'static Reader {
    READER.get_or_init(|| {
        let (tx, rx) = sync_channel::<(Req, SyncSender<Rep>)>(8);
        std::thread::Builder::new()
            .name("uia-worker".into())
            .spawn(move || worker_main(rx))   // COM + IUIAutomation live here forever
            .expect("spawn uia-worker");
        Reader { tx }
    })
}

fn call(req: Req, timeout_ms: u64) -> Option<Rep> {
    let (rtx, rrx) = sync_channel(1);
    reader().tx.try_send((req, rtx)).ok()?;
    rrx.recv_timeout(Duration::from_millis(timeout_ms)).ok()
}

// public helpers used by imp_windows.rs
pub fn value_any(timeout_ms: u64) -> Option<String> {
    match call(Req::ValueAny, timeout_ms)? { Rep::Text(t) => t, _ => None }
}
pub fn value_for_pid(pid: i32, timeout_ms: u64) -> Option<String> { /* … */ }
pub fn selection(timeout_ms: u64) -> Option<String> { /* … */ }
pub fn pid() -> Option<i32> { /* … */ }
pub fn activate_foreground() -> Option<i32> { /* probe + return pid */ }

fn worker_main(rx: Receiver<(Req, SyncSender<Rep>)>) {
    // CoInitializeEx(MTA) once
    // let automation = uiautomation::UIAutomation::new();
    // (optional) set IUIAutomation2 transaction/connection timeouts (best-effort)
    // let cache = build_cache_request(&automation);  // Value, IsPassword, Pid, ControlType, ValuePattern, TextPattern
    // let mut last_runtime_id: Option<Vec<i32>> = None;
    // for (req, reply) in rx { let _ = reply.try_send(read_once(...)); }
    // CoUninitialize
}
```

`imp_windows.rs` then becomes thin:

```rust
pub fn read_focused_value_fast() -> Option<String> { uia::value_any(80) }
pub fn read_focused_value_fast_for_pid(pid: i32) -> Option<String> { uia::value_for_pid(pid, 80) }
pub fn read_focused_value_first() -> Option<String> { uia::value_any(450) }       // worker does full fallback chain
pub fn read_focused_value_first_for_pid(pid: i32) -> Option<String> { uia::value_for_pid(pid, 450) }
pub fn read_selected_text() -> Option<String> { uia::selection(300).or_else(clipboard_copy_read) }
pub fn focused_pid() -> Option<i32> { uia::pid() }
pub fn unlock_focused_app_now() -> Option<i32> { uia::activate_foreground() }
pub fn lock_frontmost_app_now() -> Option<i32> { uia::activate_foreground() }
// capture_focused_text_via_selection + diagnose_focused_field below
```

Keep the worker's per-tick timeout (80ms) below the FAST poll interval (50ms is the
loop cadence; the 500ms `blocking_ax_option` is the outer guard, so 80ms inner keeps
the loop responsive). `read_focused_value_first` uses a larger budget (≈450ms) because
it does the full fallback chain.

---

## 4. UIA call mapping (per capability)

Use these `windows::Win32::UI::Accessibility` interfaces (the `uiautomation` crate
exposes typed equivalents; raw IDs given for when you drop down).

**Focused element:** `IUIAutomation::GetFocusedElement()` → `IUIAutomationElement`.
Re-resolve **every tick** — never cache the element handle (focus moves; stale handles
throw `UIA_E_ELEMENTNOTAVAILABLE`).

**Value (full field) — fallback order:**
1. `ValuePattern` (`UIA_ValuePatternId` → `CurrentValue`) — native edit/combo/`IValueProvider`.
2. `TextPattern` (`UIA_TextPatternId` → `DocumentRange().GetText(-1)`, `-1` = no cap) —
   multiline/rich/document controls.
3. **Bounded subtree walk** (Chromium/Electron contenteditable, where the top element
   has no direct value): `CreateTreeWalker` / `GetFirstChildElement` + `GetNextSiblingElement`,
   match `UIA_IsTextPatternAvailablePropertyId == true` or `ControlType ∈ {Edit, Document}`.
   **Hard caps to mirror macOS:** ≤ 64 elements visited, depth ≤ 4; short-circuit on the
   first non-empty text.

**Selected text:** `TextPattern::GetSelection()` → `IUIAutomationTextRangeArray`; take
range 0 (or concat) → `GetText(-1)`. Empty array = no selection (not an error).

**PID:** prefer `IUIAutomationElement::get_CurrentProcessId()` (the element's process —
correct for browser renderers). For the *user-facing app* identity use
`GetWindowThreadProcessId(GetForegroundWindow())`. For `*_for_pid` lock: read focused
element, return its value only if `get_CurrentProcessId() == pid`, else `None`.

**Relevant IDs:** `UIA_ValuePatternId`, `UIA_TextPatternId`, `UIA_ValueValuePropertyId`,
`UIA_IsValuePatternAvailablePropertyId`, `UIA_IsTextPatternAvailablePropertyId`,
`UIA_IsPasswordPropertyId`, `UIA_ControlTypePropertyId`, `UIA_ProcessIdPropertyId`.

---

## 5. Performance (30ms polling)

- **CacheRequest:** `CreateCacheRequest()` → `AddProperty(Value, IsPassword, ProcessId,
  ControlType)` + `AddPattern(ValuePattern, TextPattern)`. Each tick:
  `GetFocusedElement()` → `BuildUpdatedCache(&cache)` (**one** cross-process RPC for all
  props/patterns) → read from cache. This collapses N round-trips into 1.
- **Debounce focus transitions:** compare `GetRuntimeId()` / `CompareElements` to last
  tick; skip the expensive text read on transient focus (menu opening, app switching).
  Require the same runtime id for ≥2 ticks before trusting a read on the learning path.
- Native controls: sub-ms to few-ms cached reads. Browser/Electron: tens of ms — bound
  the walk hard and rely on the channel timeout.

---

## 6. Chromium / Electron activation

Chromium lazily builds its a11y tree only when an AT client is detected. Activate it
**once** when a Chrome/Electron foreground window is first seen: get the foreground
HWND and send `WM_GETOBJECT` / query its `IAccessible2` (`OBJID_CLIENT`). This flips
Chromium into accessibility mode; afterwards UIA reads of contenteditable work. Mostly
automatic on Chrome 126+ (native UIA provider), but the probe is harmless and
deterministic. This is what `unlock_focused_app_now` / `lock_frontmost_app_now` do on
Windows. Do NOT depend on `--force-renderer-accessibility` (can't control user launch).

Reading live contenteditable: focused element resolves to the inner doc node; the top
browser element usually has no ValuePattern → use TextPattern (`DocumentRange.GetText`)
and `GetSelection` for the caret/selection.

---

## 7. Edge cases & reliability

- **Password fields:** check `UIA_IsPasswordPropertyId`; if true return `None` and **do
  not** run the clipboard fallback (never capture secrets). UIA withholds the value by
  design anyway.
- **Elevated/admin windows (UIPI):** a non-elevated process can't read an elevated app's
  UI. Reads return access-denied/empty → degrade to `None` gracefully. (Crossing this
  needs Authenticode-signed + `uiAccess="true"` + install in Program Files — out of
  scope; acceptable to skip.)
- **No manifest needed** for normal use: a standard non-elevated app reading
  non-elevated windows needs no `uiAccess` and no special privileges.
- **ValuePattern unsupported:** many rich/custom controls lack it → fall through to
  TextPattern → subtree walk → clipboard.
- **Stale elements:** `UIA_E_ELEMENTNOTAVAILABLE` when focus moved → treat as `None`,
  re-resolve next tick. Wrap all element use in error handling (also covers 64-bit
  apartment-affinity invalidation).
- **Clipboard fallback details (match macOS timing):** save clipboard → Ctrl+A (≈50ms) →
  Ctrl+C (≈40ms down / 60ms up / 150ms settle) → read → restore clipboard. Destructive
  to selection + clipboard, so last resort only, and never on password fields. Clipboard
  read uses the existing Win32 clipboard helpers already in `imp_windows.rs`.

---

## 8. Cargo.toml changes

In `crates/paster/Cargo.toml`, under the existing
`[target.'cfg(target_os = "windows")'.dependencies]`:

```toml
uiautomation = "0.25"
# extend the existing `windows` features with what UIA + the probe need:
windows = { version = "0.58", features = [
    "Win32_Foundation",
    "Win32_UI_Input_KeyboardAndMouse",
    "Win32_System_DataExchange",
    "Win32_System_Memory",
    "Win32_System_Ole",
    "Win32_System_LibraryLoader",
    "Win32_System_Com",                 # CoInitializeEx / apartment
    "Win32_UI_Accessibility",           # IUIAutomation* (if calling raw)
    "Win32_UI_WindowsAndMessaging",     # GetForegroundWindow, WM_GETOBJECT, GetWindowThreadProcessId
] }
```

Verify `uiautomation` 0.25's internal `windows-rs` version is compatible; if it pins a
different major, prefer doing the raw bits through whatever `windows` version
`uiautomation` re-exports to avoid two COM-binding majors in one process.

Add `mod uia;` (gated `#[cfg(target_os = "windows")]`) to `crates/paster/src/lib.rs`.

---

## 9. Windows test plan (must run on a real Windows 10/11 box)

For each, dictate text, then hand-edit within the watch window, and confirm
`/v1/classify-edit` fires and a correction is learned (check backend `[retrain]` /
classify logs and the "Remembered your correction" toast):

1. **Notepad** — plain `ValuePattern` path.
2. **WordPad / Word** — `TextPattern` (multiline/rich) path.
3. **Chrome** — Gmail compose / a contenteditable: confirm the a11y probe activates and
   the subtree-walk reads live text.
4. **Slack / VS Code (Electron)** — Electron contenteditable path.
5. **A control with no ValuePattern** — confirm clipboard fallback (and that it restores
   the clipboard).
6. **A password field** — confirm it returns `None` and the clipboard fallback is NOT
   triggered.
7. **Focus-change mid-watch** — switch apps during the 30s; confirm clean exit / target
   lock behavior matches macOS.
8. **Performance** — confirm typing stays smooth (no jank) while the 30ms poll runs;
   watch CPU.
9. **Regression** — typing/pasting still work; no hangs when a slow app is focused.

Also add `said-paster` back into the Windows CI `cargo check`/`cargo test --lib` once it
compiles (it's already included in `ci.yml`'s `rust-windows` test leg).

---

## 10. Risks / limits (acceptable)

- Can't read elevated/admin windows without the signed+uiAccess route (skip).
- Browser/Electron reads are heavier than native; bounded walk + channel timeout keep the
  hot path safe.
- UIA's own timeouts are unreliable — the caller-side `recv_timeout` is the real guard.
- Password fields are intentionally unreadable (correct behavior).

Everything else reaches macOS parity. After this lands, Windows learning is identical to
macOS: same capture methods, same 4-way classifier, same 3 gates, same persistence.
