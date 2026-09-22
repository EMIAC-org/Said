//! Tap-to-talk on top of hold-to-talk, for record hotkeys that only report
//! "down" and "up".
//!
//! Hold the key: recording runs while it is down and is processed on release,
//! as before. Tap it quickly: recording keeps running, and the next press
//! finishes it. Caps Lock on macOS never needed this — the OS toggles its state
//! on every press, so taps already latched — which is why switching the record
//! key to Fn or a modifier looked like tap mode had broken.
//!
//! A press that is used as a shortcut (another key goes down while the hotkey
//! is held, e.g. Fn+arrow) is never a tap: its release finishes as before, so a
//! quick shortcut can't leave the microphone running.
//!
//! Pure and platform-free so both the macOS and Windows hooks share it and it
//! can be tested without a keyboard.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A release sooner than this after the press counts as a tap.
pub const TAP_MAX: Duration = Duration::from_millis(350);

/// What the platform hook should do with a record-hotkey event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LatchAction {
    /// Start recording (fire the press callback).
    Start,
    /// Stop and process (fire the release callback).
    Finish,
    /// Nothing to fire.
    Ignore,
}

#[derive(Debug)]
pub struct TapLatch {
    /// A tap started a recording that is still running.
    latched: bool,
    /// When the current press began; `None` while the key is up.
    pressed_at: Option<Instant>,
    /// Another key went down during the current press.
    tainted: bool,
}

impl TapLatch {
    pub const fn new() -> Self {
        Self {
            latched: false,
            pressed_at: None,
            tainted: false,
        }
    }

    pub fn press(&mut self, now: Instant) -> LatchAction {
        if self.latched {
            // The second tap ends the latched recording. Its own release is
            // then ignored because `pressed_at` stays empty.
            self.latched = false;
            return LatchAction::Finish;
        }
        self.pressed_at = Some(now);
        self.tainted = false;
        LatchAction::Start
    }

    pub fn release(&mut self, now: Instant) -> LatchAction {
        let Some(pressed_at) = self.pressed_at.take() else {
            return LatchAction::Ignore;
        };
        if !self.tainted && now.saturating_duration_since(pressed_at) < TAP_MAX {
            self.latched = true;
            return LatchAction::Ignore;
        }
        LatchAction::Finish
    }

    /// Any other key went down. Marks the current press as a shortcut.
    pub fn other_key(&mut self) {
        if self.pressed_at.is_some() {
            self.tainted = true;
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

impl Default for TapLatch {
    fn default() -> Self {
        Self::new()
    }
}

static LATCH: Mutex<TapLatch> = Mutex::new(TapLatch::new());

pub(crate) fn with_latch<R>(f: impl FnOnce(&mut TapLatch) -> R) -> R {
    let mut latch = LATCH
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    f(&mut latch)
}

/// Forget a latched tap. Call whenever a recording ends by any route other than
/// the hotkey (cancel, error, time limit), or the next press would try to
/// finish a recording that no longer exists instead of starting a new one.
pub fn reset_tap_latch() {
    with_latch(TapLatch::reset);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(ms: u64, base: Instant) -> Instant {
        base + Duration::from_millis(ms)
    }

    #[test]
    fn a_hold_starts_on_press_and_finishes_on_release() {
        let t = Instant::now();
        let mut latch = TapLatch::new();
        assert_eq!(latch.press(t), LatchAction::Start);
        assert_eq!(latch.release(at(900, t)), LatchAction::Finish);
    }

    #[test]
    fn a_tap_keeps_recording_until_the_next_press() {
        let t = Instant::now();
        let mut latch = TapLatch::new();
        assert_eq!(latch.press(t), LatchAction::Start);
        assert_eq!(latch.release(at(90, t)), LatchAction::Ignore);
        assert_eq!(latch.press(at(4_000, t)), LatchAction::Finish);
        // The finishing press's own release does nothing.
        assert_eq!(latch.release(at(4_080, t)), LatchAction::Ignore);
        // And the cycle starts over cleanly.
        assert_eq!(latch.press(at(6_000, t)), LatchAction::Start);
    }

    #[test]
    fn a_long_second_press_still_just_finishes() {
        let t = Instant::now();
        let mut latch = TapLatch::new();
        latch.press(t);
        latch.release(at(100, t));
        assert_eq!(latch.press(at(3_000, t)), LatchAction::Finish);
        assert_eq!(latch.release(at(5_000, t)), LatchAction::Ignore);
    }

    #[test]
    fn a_quick_shortcut_is_not_a_tap() {
        // Fn+arrow: another key during a short press must not leave the mic on.
        let t = Instant::now();
        let mut latch = TapLatch::new();
        latch.press(t);
        latch.other_key();
        assert_eq!(latch.release(at(120, t)), LatchAction::Finish);
        assert_eq!(latch.press(at(2_000, t)), LatchAction::Start);
    }

    #[test]
    fn other_keys_while_latched_do_not_matter() {
        // Keys pressed while the hotkey is up (between the two taps) must not
        // turn the finishing press into a shortcut.
        let t = Instant::now();
        let mut latch = TapLatch::new();
        latch.press(t);
        latch.release(at(80, t));
        latch.other_key();
        assert_eq!(latch.press(at(1_000, t)), LatchAction::Finish);
    }

    #[test]
    fn the_threshold_is_exclusive() {
        let t = Instant::now();
        let mut latch = TapLatch::new();
        latch.press(t);
        assert_eq!(
            latch.release(t + TAP_MAX),
            LatchAction::Finish,
            "a press exactly TAP_MAX long is a hold"
        );
    }

    #[test]
    fn reset_forgets_a_latched_tap() {
        // The recording ended some other way; the next press must start anew.
        let t = Instant::now();
        let mut latch = TapLatch::new();
        latch.press(t);
        latch.release(at(90, t));
        latch.reset();
        assert_eq!(latch.press(at(2_000, t)), LatchAction::Start);
    }

    #[test]
    fn a_stray_release_is_ignored() {
        let mut latch = TapLatch::new();
        assert_eq!(latch.release(Instant::now()), LatchAction::Ignore);
    }
}
