//! Pure logic for the Windows low-level keyboard hook.
//!
//! Compiled on every platform so its unit tests run as part of the normal
//! `cargo test` flow on macOS / Linux / Windows. The actual Win32 plumbing
//! lives in [`crate::imp_windows`] and is gated by `cfg(target_os = "windows")`.
//!
//! Splitting it this way means we can verify the swallow / fire-once /
//! key-up-without-keydown invariants without ever calling `SetWindowsHookExW`,
//! which is what makes regression coverage of this logic possible on a Mac
//! dev box and on every CI runner.

use crate::RecordHotkey;

/// Windows virtual-key codes we care about. Hardcoded here (rather than imported
/// from the `windows` crate) so this module compiles on every host. The values
/// are from the public Microsoft VK enumeration and are stable.
pub const VK_CAPITAL: u32 = 0x14;
pub const VK_RMENU: u32 = 0xA5;
/// Number-row digit virtual-key codes. Combined with held Ctrl+Shift these
/// fire the tone-polish shortcuts (Windows analog of Mac's ⌥1..⌥5; Alt
/// would activate the menu bar in Chromium browsers + classic Win32 apps).
pub const VK_1: u32 = 0x31;
pub const VK_2: u32 = 0x32;
pub const VK_3: u32 = 0x33;
pub const VK_4: u32 = 0x34;
pub const VK_5: u32 = 0x35;

/// Win32 message identifiers for keyboard events. Same stability guarantee.
pub const WM_KEYDOWN: u32 = 0x0100;
pub const WM_KEYUP: u32 = 0x0101;
pub const WM_SYSKEYDOWN: u32 = 0x0104;
pub const WM_SYSKEYUP: u32 = 0x0105;

/// The hook proc's interpretation of a `wparam` value.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum EvtKind {
    KeyDown,
    KeyUp,
    Other,
}

/// What the hook should do for a given event.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum HookAction {
    /// Forward to the next hook in the chain (return `CallNextHookEx`).
    PassThrough,
    /// Suppress the event (return `LRESULT(1)`). For Caps Lock keydown/keyup we
    /// swallow so the OS-level toggle state never flips.
    Swallow {
        fire_press: bool,
        fire_release: bool,
    },
}

/// Map a Win32 wparam value to an [`EvtKind`].
pub fn wparam_to_kind(wparam: u32) -> EvtKind {
    if wparam == WM_KEYDOWN || wparam == WM_SYSKEYDOWN {
        EvtKind::KeyDown
    } else if wparam == WM_KEYUP || wparam == WM_SYSKEYUP {
        EvtKind::KeyUp
    } else {
        EvtKind::Other
    }
}

/// Map a [`RecordHotkey`] to the Windows VK we watch.
/// Returns `None` for hotkeys with no Windows analog (the macOS Fn / Globe
/// key — PCs don't expose a single public VK for the function modifier).
pub fn target_vk(hotkey: RecordHotkey) -> Option<u32> {
    match hotkey {
        RecordHotkey::CapsLock => Some(VK_CAPITAL),
        RecordHotkey::RightOption => Some(VK_RMENU),
        RecordHotkey::Function => None,
    }
}

/// Pure: decide what the hook should do given the current state and event.
///
/// `was_down` is the cached IS_DOWN flag from the caller — the caller flips
/// it to match the returned `fire_press` / `fire_release` flags before
/// invoking the user callbacks.
pub fn classify(vk: u32, kind: EvtKind, target: Option<u32>, was_down: bool) -> HookAction {
    let target = match target {
        Some(t) => t,
        None => return HookAction::PassThrough,
    };

    if vk != target {
        return HookAction::PassThrough;
    }

    match kind {
        EvtKind::KeyDown => HookAction::Swallow {
            fire_press: !was_down,
            fire_release: false,
        },
        EvtKind::KeyUp => HookAction::Swallow {
            fire_press: false,
            fire_release: was_down,
        },
        EvtKind::Other => HookAction::PassThrough,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wparam_maps_to_event_kind() {
        assert_eq!(wparam_to_kind(WM_KEYDOWN), EvtKind::KeyDown);
        assert_eq!(wparam_to_kind(WM_KEYUP), EvtKind::KeyUp);
        assert_eq!(wparam_to_kind(WM_SYSKEYDOWN), EvtKind::KeyDown);
        assert_eq!(wparam_to_kind(WM_SYSKEYUP), EvtKind::KeyUp);
        assert_eq!(wparam_to_kind(0xDEAD), EvtKind::Other);
        assert_eq!(wparam_to_kind(0), EvtKind::Other);
    }

    #[test]
    fn target_vk_for_each_record_hotkey() {
        assert_eq!(target_vk(RecordHotkey::CapsLock), Some(VK_CAPITAL));
        assert_eq!(target_vk(RecordHotkey::RightOption), Some(VK_RMENU));
        assert_eq!(target_vk(RecordHotkey::Function), None);
    }

    #[test]
    fn classify_passes_through_non_target_keys() {
        // 0x41 == 'A' — never the hotkey.
        for kind in [EvtKind::KeyDown, EvtKind::KeyUp, EvtKind::Other] {
            for was_down in [true, false] {
                assert_eq!(
                    classify(0x41, kind, Some(VK_CAPITAL), was_down),
                    HookAction::PassThrough,
                    "non-target key must always pass through — vk=0x41 kind={kind:?} was_down={was_down}"
                );
            }
        }
    }

    #[test]
    fn classify_passes_through_when_target_is_none() {
        // Function-key binding has no Windows analog — everything passes
        // through regardless of vk.
        assert_eq!(
            classify(VK_CAPITAL, EvtKind::KeyDown, None, false),
            HookAction::PassThrough
        );
        assert_eq!(
            classify(VK_RMENU, EvtKind::KeyUp, None, true),
            HookAction::PassThrough
        );
    }

    #[test]
    fn caps_lock_keydown_first_time_fires_press_and_swallows() {
        assert_eq!(
            classify(VK_CAPITAL, EvtKind::KeyDown, Some(VK_CAPITAL), false),
            HookAction::Swallow {
                fire_press: true,
                fire_release: false
            }
        );
    }

    #[test]
    fn caps_lock_keydown_while_held_is_swallowed_but_does_not_re_fire() {
        // Low-level hook fires on autorepeat — must not invoke on_press more than once.
        assert_eq!(
            classify(VK_CAPITAL, EvtKind::KeyDown, Some(VK_CAPITAL), true),
            HookAction::Swallow {
                fire_press: false,
                fire_release: false
            }
        );
    }

    #[test]
    fn caps_lock_keyup_while_down_fires_release_and_swallows() {
        assert_eq!(
            classify(VK_CAPITAL, EvtKind::KeyUp, Some(VK_CAPITAL), true),
            HookAction::Swallow {
                fire_press: false,
                fire_release: true
            }
        );
    }

    #[test]
    fn caps_lock_spurious_keyup_is_swallowed_but_does_not_fire_release() {
        // Spurious keyup with no prior keydown (e.g. focus change races) —
        // still swallow the toggle but don't double-fire release.
        assert_eq!(
            classify(VK_CAPITAL, EvtKind::KeyUp, Some(VK_CAPITAL), false),
            HookAction::Swallow {
                fire_press: false,
                fire_release: false
            }
        );
    }

    #[test]
    fn caps_lock_other_message_is_swallowed_without_firing() {
        // Hooks can deliver odd wparam values (e.g. WM_INPUT) — when the vk
        // matches the target but the message kind is neither down nor up,
        // pass through so we don't accidentally suppress some other event.
        assert_eq!(
            classify(VK_CAPITAL, EvtKind::Other, Some(VK_CAPITAL), true),
            HookAction::PassThrough
        );
    }

    #[test]
    fn right_alt_binding_swallows_right_alt_only() {
        let target = Some(VK_RMENU);
        // Right Alt down fires press
        assert_eq!(
            classify(VK_RMENU, EvtKind::KeyDown, target, false),
            HookAction::Swallow {
                fire_press: true,
                fire_release: false
            }
        );
        // Caps Lock pressed while bound to Right Alt — pass through
        assert_eq!(
            classify(VK_CAPITAL, EvtKind::KeyDown, target, false),
            HookAction::PassThrough
        );
        // Right Alt up fires release
        assert_eq!(
            classify(VK_RMENU, EvtKind::KeyUp, target, true),
            HookAction::Swallow {
                fire_press: false,
                fire_release: true
            }
        );
    }

    /// End-to-end state machine: simulate a full keydown → autorepeat → keyup
    /// cycle and verify on_press fires exactly once and on_release fires
    /// exactly once. Every Caps Lock event in the cycle must be swallowed —
    /// the whole point of the hook is that the OS-level Caps Lock toggle
    /// never fires. The press/release callbacks fire selectively.
    #[test]
    fn full_hold_cycle_fires_press_and_release_exactly_once() {
        let target = Some(VK_CAPITAL);
        let mut down = false;
        let mut presses = 0;
        let mut releases = 0;

        let events = [
            EvtKind::KeyDown, // initial press
            EvtKind::KeyDown, // autorepeat
            EvtKind::KeyDown, // autorepeat
            EvtKind::KeyDown, // autorepeat
            EvtKind::KeyUp,   // release
            EvtKind::KeyUp,   // spurious second up
        ];

        for kind in events {
            match classify(VK_CAPITAL, kind, target, down) {
                HookAction::Swallow {
                    fire_press,
                    fire_release,
                } => {
                    if fire_press {
                        presses += 1;
                        down = true;
                    }
                    if fire_release {
                        releases += 1;
                        down = false;
                    }
                }
                HookAction::PassThrough => {
                    panic!(
                        "Caps Lock event must always be swallowed to prevent OS toggle — kind={kind:?} down={down}"
                    );
                }
            }
        }

        assert_eq!(
            presses, 1,
            "on_press must fire exactly once across the hold cycle"
        );
        assert_eq!(
            releases, 1,
            "on_release must fire exactly once across the hold cycle"
        );
        assert!(!down, "IS_DOWN must end up false after the keyup");
    }
}

// ── Tone-polish shortcut detection (Ctrl+Shift+1 .. Ctrl+Shift+5) ────────────
//
// The Mac path uses Option+digit (CGEventTap). On Windows we deliberately
// avoid Alt+digit because:
//   - Pressing Alt alone activates the menu bar in Chromium browsers (Chrome,
//     Edge, Brave, Slack, VS Code) — bumping focus off the user's text
//     selection before our hook's worker can run.
//   - Alt+digit triggers menu-bar accelerators in every classic Win32 app
//     with a menu (Office, Notepad++, etc.).
//
// Ctrl+Shift+digit has none of those collisions. It's the idiomatic Windows
// pattern for app-specific global chords. The hook intercepts on digit
// keydown, checks Ctrl + Shift are held (Win/Alt MUST NOT be) and fires the
// shortcut callback with the digit. We swallow the digit event so the focused
// app doesn't also receive a Ctrl+Shift+digit (most apps have nothing on
// that chord; swallow is defense-in-depth).

/// Modifier state captured at the moment a digit key was pressed.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub struct ModifierState {
    pub left_alt: bool,
    pub right_alt: bool,
    pub ctrl: bool,
    pub shift: bool,
    pub win: bool,
}

/// Pure: return `Some(digit)` if this event should fire the `1..=5`
/// tone-polish shortcut. The caller swallows the event when this returns
/// `Some` so the focused app doesn't double-receive the chord.
///
/// - Only the keydown is meaningful — autorepeat / keyup must not re-fire.
/// - The vk must be in `VK_1..=VK_5`.
/// - Modifiers must be exactly Ctrl + Shift (no Alt, no Win).
pub fn classify_shortcut(vk: u32, kind: EvtKind, mods: ModifierState) -> Option<u8> {
    if kind != EvtKind::KeyDown {
        return None;
    }
    if !(VK_1..=VK_5).contains(&vk) {
        return None;
    }
    if !mods.ctrl || !mods.shift {
        return None;
    }
    if mods.left_alt || mods.right_alt || mods.win {
        return None;
    }
    let digit = (vk - 0x30) as u8;
    Some(digit)
}

#[cfg(test)]
mod shortcut_tests {
    use super::*;

    fn ctrl_shift() -> ModifierState {
        ModifierState {
            ctrl: true,
            shift: true,
            ..Default::default()
        }
    }

    #[test]
    fn ctrl_shift_1_through_5_fire_corresponding_digit() {
        for (vk, expected) in [(VK_1, 1), (VK_2, 2), (VK_3, 3), (VK_4, 4), (VK_5, 5)] {
            assert_eq!(
                classify_shortcut(vk, EvtKind::KeyDown, ctrl_shift()),
                Some(expected as u8),
                "Ctrl+Shift+{expected} should fire shortcut {expected}"
            );
        }
    }

    #[test]
    fn ctrl_shift_0_and_ctrl_shift_6_do_not_fire() {
        // VK_0 = 0x30, VK_6 = 0x36 — outside the supported range.
        assert_eq!(
            classify_shortcut(0x30, EvtKind::KeyDown, ctrl_shift()),
            None
        );
        assert_eq!(
            classify_shortcut(0x36, EvtKind::KeyDown, ctrl_shift()),
            None
        );
    }

    #[test]
    fn keyup_never_fires() {
        // The Mac path fires on keydown only; matching that here matters
        // because the hook receives both events for every key.
        assert_eq!(classify_shortcut(VK_1, EvtKind::KeyUp, ctrl_shift()), None);
    }

    #[test]
    fn other_event_kinds_do_not_fire() {
        assert_eq!(classify_shortcut(VK_1, EvtKind::Other, ctrl_shift()), None);
    }

    #[test]
    fn ctrl_only_does_not_fire() {
        // Ctrl+digit on its own is reserved by browsers (Ctrl+1..9 jumps to
        // the Nth tab) and many editors. We do not poach it.
        let ctrl_only = ModifierState {
            ctrl: true,
            ..Default::default()
        };
        assert_eq!(classify_shortcut(VK_1, EvtKind::KeyDown, ctrl_only), None);
    }

    #[test]
    fn shift_only_does_not_fire() {
        // Shift+digit is just the symbol on top of the digit key (!@#$%).
        let shift_only = ModifierState {
            shift: true,
            ..Default::default()
        };
        assert_eq!(classify_shortcut(VK_1, EvtKind::KeyDown, shift_only), None);
    }

    #[test]
    fn ctrl_shift_plus_alt_or_win_does_not_fire() {
        // Ctrl+Shift+Alt+digit and Ctrl+Shift+Win+digit are reserved for
        // user-customised macros / IME hotkeys / window-manager bindings.
        for extra in [
            ModifierState {
                ctrl: true,
                shift: true,
                left_alt: true,
                ..Default::default()
            },
            ModifierState {
                ctrl: true,
                shift: true,
                right_alt: true,
                ..Default::default()
            },
            ModifierState {
                ctrl: true,
                shift: true,
                win: true,
                ..Default::default()
            },
        ] {
            assert_eq!(
                classify_shortcut(VK_1, EvtKind::KeyDown, extra),
                None,
                "Ctrl+Shift + other modifier must not fire: {extra:?}"
            );
        }
    }

    #[test]
    fn no_modifiers_does_not_fire() {
        // A bare "1" keystroke (typing the digit one) must pass through.
        assert_eq!(
            classify_shortcut(VK_1, EvtKind::KeyDown, ModifierState::default()),
            None
        );
    }
}
