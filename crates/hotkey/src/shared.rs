//! Cross-platform types shared between the macOS and Windows implementations.
//!
//! `KeyEvt`, `TimedKeyEvt`, and the global `KEY_BUF` live here so that
//! `key_buffer()` is callable on every supported OS — Windows returns an
//! empty buffer until the platform implementation populates it.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

/// A compact, clonable keystroke event routed from the platform input hook.
#[derive(Clone, Debug)]
pub enum KeyEvt {
    Char(char),    // printable character produced by this keypress
    Backspace,     // delete char before cursor (plain)
    Delete,        // delete char after cursor (plain)
    Left,          // move cursor one char left
    Right,         // move cursor one char right
    Home,          // jump to line/text start
    End,           // jump to line/text end
    WordLeft,      // Option+Left  — jump to previous word start
    WordRight,     // Option+Right — jump to next word end
    LineStart,     // Cmd+Left     — jump to line start
    LineEnd,       // Cmd+Right    — jump to line end
    WordBackspace, // Option+Backspace — delete word before cursor
    LineBackspace, // Cmd+Backspace    — delete to line start
    SelectAll,     // Cmd+A
    Cut,           // Cmd+X — marks reconstruction uncertain
    Undo,          // Cmd+Z — marks reconstruction uncertain
    MouseClick,    // mouse repositioned cursor — uncertain
    Other,         // ignored
}

/// Timestamped key event stored in the ring buffer.
pub struct TimedKeyEvt {
    pub when: Instant,
    pub evt: KeyEvt,
}

/// Global rolling buffer capacity (~2000 most recent events).
pub(crate) const KEY_BUF_CAPACITY: usize = 2048;

static KEY_BUF: OnceLock<Arc<Mutex<VecDeque<TimedKeyEvt>>>> = OnceLock::new();

pub(crate) fn key_buf() -> &'static Arc<Mutex<VecDeque<TimedKeyEvt>>> {
    KEY_BUF.get_or_init(|| Arc::new(Mutex::new(VecDeque::with_capacity(KEY_BUF_CAPACITY))))
}

/// Returns a handle to the global key-event buffer.
///
/// Consumers (e.g. the edit-watch reconstruction in Tauri) snapshot
/// `Instant::now()` before watching, then drain events newer than that
/// instant from the returned mutex.
pub fn key_buffer() -> Arc<Mutex<VecDeque<TimedKeyEvt>>> {
    Arc::clone(key_buf())
}

/// Push a key event into the ring buffer. Called from each platform's input
/// hook callback. Drops the oldest event if the buffer is at capacity.
pub(crate) fn push_key(evt: KeyEvt) {
    let buf = key_buf();
    if let Ok(mut g) = buf.lock() {
        if g.len() >= KEY_BUF_CAPACITY {
            g.pop_front();
        }
        g.push_back(TimedKeyEvt {
            when: Instant::now(),
            evt,
        });
    }
}
