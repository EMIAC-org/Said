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

use crate::{HudShortcutAction, RecordHotkey};

/// Windows virtual-key codes we care about. Hardcoded here (rather than imported
/// from the `windows` crate) so this module compiles on every host. The values
/// are from the public Microsoft VK enumeration and are stable.
pub const VK_CAPITAL: u32 = 0x14;
pub const VK_CONTROL: u32 = 0x11;
pub const VK_SHIFT: u32 = 0x10;
pub const VK_MENU: u32 = 0x12;
pub const VK_SPACE: u32 = 0x20;
pub const VK_1: u32 = 0x31;
pub const VK_2: u32 = 0x32;
pub const VK_3: u32 = 0x33;
pub const VK_4: u32 = 0x34;
pub const VK_5: u32 = 0x35;
pub const VK_N: u32 = 0x4E;
pub const VK_V: u32 = 0x56;
pub const VK_R: u32 = 0x52;
pub const VK_LCONTROL: u32 = 0xA2;
pub const VK_RCONTROL: u32 = 0xA3;
pub const VK_LMENU: u32 = 0xA4;
pub const VK_RMENU: u32 = 0xA5;
pub const VK_OEM_PERIOD: u32 = 0xBE;
pub const VK_OEM_2: u32 = 0xBF; // US keyboard "/?" key.

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

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub struct WinModifiers {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub left_alt: bool,
    pub right_alt: bool,
}

impl WinModifiers {
    pub fn none() -> Self {
        Self::default()
    }

    pub fn left_alt() -> Self {
        Self {
            alt: true,
            left_alt: true,
            ..Self::default()
        }
    }

    pub fn ctrl_left_alt() -> Self {
        Self {
            ctrl: true,
            alt: true,
            left_alt: true,
            ..Self::default()
        }
    }

    pub fn ctrl_shift() -> Self {
        Self {
            ctrl: true,
            shift: true,
            ..Self::default()
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ShortcutAction {
    Tone(u8),
    PasteLatest,
    Hud(HudShortcutAction),
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub struct DivoSnapshot {
    pub is_down: bool,
    pub tainted: bool,
    pub started: bool,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum DivoEffect {
    None,
    StartTimer,
    MarkNewChat,
    MarkTainted,
    Release,
    Cancel,
    ClearTap,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct DivoDecision {
    pub next: DivoSnapshot,
    pub effect: DivoEffect,
    pub swallow: bool,
    pub bump_generation: bool,
}

impl DivoDecision {
    fn none(state: DivoSnapshot) -> Self {
        Self {
            next: state,
            effect: DivoEffect::None,
            swallow: false,
            bump_generation: false,
        }
    }
}

/// Match the macOS Divo hold delay. Kept in the pure module so the behavior has
/// one tested contract across platform plumbing.
pub const DIVO_HOLD_DELAY_MS: u64 = 280;

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
        // Any sided modifier maps straight to its VK; the hook's keydown/keyup +
        // was_down tracking already gives correct hold/release for any VK.
        RecordHotkey::Modifier { win_vk, .. } => Some(win_vk),
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

pub fn is_control_vk(vk: u32) -> bool {
    matches!(vk, VK_CONTROL | VK_LCONTROL | VK_RCONTROL)
}

pub fn classify_divo_event(
    vk: u32,
    kind: EvtKind,
    enabled: bool,
    state: DivoSnapshot,
) -> DivoDecision {
    if !enabled {
        return DivoDecision::none(state);
    }

    match kind {
        EvtKind::KeyDown if is_control_vk(vk) => {
            if state.is_down {
                return DivoDecision::none(state);
            }
            DivoDecision {
                next: DivoSnapshot {
                    is_down: true,
                    tainted: false,
                    started: false,
                },
                effect: DivoEffect::StartTimer,
                swallow: false,
                bump_generation: true,
            }
        }
        EvtKind::KeyDown if state.is_down => {
            if vk == VK_N {
                DivoDecision {
                    next: state,
                    effect: DivoEffect::MarkNewChat,
                    swallow: true,
                    bump_generation: false,
                }
            } else {
                DivoDecision {
                    next: DivoSnapshot {
                        tainted: true,
                        ..state
                    },
                    effect: DivoEffect::MarkTainted,
                    swallow: false,
                    bump_generation: false,
                }
            }
        }
        EvtKind::KeyUp if is_control_vk(vk) && state.is_down => {
            let mut next = DivoSnapshot {
                is_down: false,
                started: false,
                ..state
            };
            let effect = if state.started {
                if state.tainted {
                    next.tainted = false;
                    DivoEffect::Cancel
                } else {
                    DivoEffect::Release
                }
            } else {
                next.tainted = false;
                DivoEffect::ClearTap
            };
            DivoDecision {
                next,
                effect,
                swallow: false,
                bump_generation: true,
            }
        }
        _ => DivoDecision::none(state),
    }
}

pub fn classify_shortcut(
    vk: u32,
    kind: EvtKind,
    mods: WinModifiers,
    record_hotkey: RecordHotkey,
) -> Option<ShortcutAction> {
    if kind != EvtKind::KeyDown {
        return None;
    }

    // macOS uses bare Option+1..5 for tray tone polish. Windows maps that to
    // bare Alt+1..5, but when Right Alt is the hold-to-record key we reserve it
    // and only accept Left Alt for tone shortcuts.
    if mods.alt && !mods.ctrl && !mods.shift {
        if matches!(record_hotkey, RecordHotkey::RightOption) && (!mods.left_alt || mods.right_alt)
        {
            return None;
        }
        let tone = match vk {
            VK_1 => 1,
            VK_2 => 2,
            VK_3 => 3,
            VK_4 => 4,
            VK_5 => 5,
            _ => return None,
        };
        return Some(ShortcutAction::Tone(tone));
    }

    // Paste-latest mirrors macOS Ctrl+Cmd+V using Ctrl+LeftAlt+V. Right Alt is
    // excluded so AltGr keyboard layouts can keep producing text.
    if mods.ctrl && mods.alt && mods.left_alt && !mods.right_alt && !mods.shift && vk == VK_V {
        return Some(ShortcutAction::PasteLatest);
    }

    // Retry-last-from-audio: Ctrl+Left-Alt+R. Mirrors paste-latest's modifier
    // choice (left-alt only so AltGr layouts keep producing text) and stays
    // conflict-free vs browser shortcuts.
    if mods.ctrl && mods.alt && mods.left_alt && !mods.right_alt && !mods.shift && vk == VK_R {
        return Some(ShortcutAction::Hud(HudShortcutAction::RetryLastFromAudio));
    }

    // HUD shortcuts use Ctrl+Shift on Windows because the status bar already
    // advertises Shift+Ctrl+/ for placement mode.
    if mods.ctrl && mods.shift && !mods.alt {
        let action = match vk {
            VK_OEM_2 => HudShortcutAction::PlacementMode,
            VK_OEM_PERIOD => HudShortcutAction::ResetPosition,
            VK_SPACE => HudShortcutAction::ToggleMessagePolishMode,
            _ => return None,
        };
        return Some(ShortcutAction::Hud(action));
    }

    None
}

pub fn classify_long_dictation(
    vk: u32,
    kind: EvtKind,
    mods: WinModifiers,
    record_hotkey: RecordHotkey,
    record_key_down: bool,
) -> bool {
    if kind != EvtKind::KeyDown || vk != VK_SPACE || !record_key_down {
        return false;
    }

    match record_hotkey {
        RecordHotkey::CapsLock => !mods.ctrl && !mods.shift && !mods.alt,
        RecordHotkey::RightOption => {
            mods.alt && mods.right_alt && !mods.left_alt && !mods.ctrl && !mods.shift
        }
        RecordHotkey::Function => false,
        // Space-to-lock long dictation is only wired for the preset keys; a custom
        // sided modifier records normally but doesn't gate the long-dictation lock.
        RecordHotkey::Modifier { .. } => false,
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
    fn divo_classifier_ignores_events_when_disabled() {
        let state = DivoSnapshot::default();
        assert_eq!(
            classify_divo_event(VK_CONTROL, EvtKind::KeyDown, false, state),
            DivoDecision::none(state)
        );
    }

    #[test]
    fn divo_control_keydown_starts_delayed_hold_once() {
        let state = DivoSnapshot::default();
        assert_eq!(
            classify_divo_event(VK_LCONTROL, EvtKind::KeyDown, true, state),
            DivoDecision {
                next: DivoSnapshot {
                    is_down: true,
                    tainted: false,
                    started: false,
                },
                effect: DivoEffect::StartTimer,
                swallow: false,
                bump_generation: true,
            }
        );

        let down = DivoSnapshot {
            is_down: true,
            ..DivoSnapshot::default()
        };
        assert_eq!(
            classify_divo_event(VK_RCONTROL, EvtKind::KeyDown, true, down),
            DivoDecision::none(down)
        );
    }

    #[test]
    fn divo_quick_control_tap_clears_without_callbacks() {
        let down = DivoSnapshot {
            is_down: true,
            ..DivoSnapshot::default()
        };
        assert_eq!(
            classify_divo_event(VK_CONTROL, EvtKind::KeyUp, true, down),
            DivoDecision {
                next: DivoSnapshot::default(),
                effect: DivoEffect::ClearTap,
                swallow: false,
                bump_generation: true,
            }
        );
    }

    #[test]
    fn divo_ctrl_n_records_new_chat_and_swallows_key() {
        let down = DivoSnapshot {
            is_down: true,
            started: true,
            ..DivoSnapshot::default()
        };
        assert_eq!(
            classify_divo_event(VK_N, EvtKind::KeyDown, true, down),
            DivoDecision {
                next: down,
                effect: DivoEffect::MarkNewChat,
                swallow: true,
                bump_generation: false,
            }
        );
    }

    #[test]
    fn divo_other_key_while_down_taints_without_swallowing() {
        let down = DivoSnapshot {
            is_down: true,
            started: true,
            ..DivoSnapshot::default()
        };
        assert_eq!(
            classify_divo_event(VK_V, EvtKind::KeyDown, true, down),
            DivoDecision {
                next: DivoSnapshot {
                    tainted: true,
                    ..down
                },
                effect: DivoEffect::MarkTainted,
                swallow: false,
                bump_generation: false,
            }
        );
    }

    #[test]
    fn divo_release_after_started_hold_fires_release_or_cancel() {
        let clean = DivoSnapshot {
            is_down: true,
            started: true,
            tainted: false,
        };
        assert_eq!(
            classify_divo_event(VK_RCONTROL, EvtKind::KeyUp, true, clean),
            DivoDecision {
                next: DivoSnapshot::default(),
                effect: DivoEffect::Release,
                swallow: false,
                bump_generation: true,
            }
        );

        let tainted = DivoSnapshot {
            tainted: true,
            ..clean
        };
        assert_eq!(
            classify_divo_event(VK_RCONTROL, EvtKind::KeyUp, true, tainted),
            DivoDecision {
                next: DivoSnapshot::default(),
                effect: DivoEffect::Cancel,
                swallow: false,
                bump_generation: true,
            }
        );
    }

    #[test]
    fn shortcut_classifier_ignores_keyup_and_unmodified_keys() {
        assert_eq!(
            classify_shortcut(
                VK_1,
                EvtKind::KeyUp,
                WinModifiers::left_alt(),
                RecordHotkey::CapsLock,
            ),
            None
        );
        assert_eq!(
            classify_shortcut(
                VK_1,
                EvtKind::KeyDown,
                WinModifiers::none(),
                RecordHotkey::CapsLock,
            ),
            None
        );
    }

    #[test]
    fn alt_digits_fire_tone_shortcuts() {
        for (vk, tone) in [(VK_1, 1), (VK_2, 2), (VK_3, 3), (VK_4, 4), (VK_5, 5)] {
            assert_eq!(
                classify_shortcut(
                    vk,
                    EvtKind::KeyDown,
                    WinModifiers::left_alt(),
                    RecordHotkey::CapsLock,
                ),
                Some(ShortcutAction::Tone(tone))
            );
        }
    }

    #[test]
    fn tone_shortcuts_require_bare_alt_and_known_digits() {
        assert_eq!(
            classify_shortcut(
                VK_1,
                EvtKind::KeyDown,
                WinModifiers {
                    ctrl: true,
                    ..WinModifiers::left_alt()
                },
                RecordHotkey::CapsLock,
            ),
            None
        );
        assert_eq!(
            classify_shortcut(
                VK_V,
                EvtKind::KeyDown,
                WinModifiers::left_alt(),
                RecordHotkey::CapsLock,
            ),
            None
        );
    }

    #[test]
    fn right_alt_record_hotkey_reserves_right_alt_from_tones() {
        assert_eq!(
            classify_shortcut(
                VK_1,
                EvtKind::KeyDown,
                WinModifiers::left_alt(),
                RecordHotkey::RightOption,
            ),
            Some(ShortcutAction::Tone(1))
        );
        assert_eq!(
            classify_shortcut(
                VK_1,
                EvtKind::KeyDown,
                WinModifiers {
                    alt: true,
                    right_alt: true,
                    ..WinModifiers::default()
                },
                RecordHotkey::RightOption,
            ),
            None
        );
    }

    #[test]
    fn ctrl_left_alt_v_fires_paste_latest() {
        assert_eq!(
            classify_shortcut(
                VK_V,
                EvtKind::KeyDown,
                WinModifiers::ctrl_left_alt(),
                RecordHotkey::CapsLock,
            ),
            Some(ShortcutAction::PasteLatest)
        );
    }

    #[test]
    fn paste_latest_rejects_shift_missing_modifiers_and_altgr() {
        assert_eq!(
            classify_shortcut(
                VK_V,
                EvtKind::KeyDown,
                WinModifiers {
                    shift: true,
                    ..WinModifiers::ctrl_left_alt()
                },
                RecordHotkey::CapsLock,
            ),
            None
        );
        assert_eq!(
            classify_shortcut(
                VK_V,
                EvtKind::KeyDown,
                WinModifiers {
                    ctrl: true,
                    alt: true,
                    right_alt: true,
                    ..WinModifiers::default()
                },
                RecordHotkey::CapsLock,
            ),
            None
        );
        assert_eq!(
            classify_shortcut(
                VK_V,
                EvtKind::KeyDown,
                WinModifiers::left_alt(),
                RecordHotkey::CapsLock,
            ),
            None
        );
    }

    #[test]
    fn ctrl_shift_hud_shortcuts_fire_expected_actions() {
        assert_eq!(
            classify_shortcut(
                VK_OEM_2,
                EvtKind::KeyDown,
                WinModifiers::ctrl_shift(),
                RecordHotkey::CapsLock,
            ),
            Some(ShortcutAction::Hud(HudShortcutAction::PlacementMode))
        );
        assert_eq!(
            classify_shortcut(
                VK_OEM_PERIOD,
                EvtKind::KeyDown,
                WinModifiers::ctrl_shift(),
                RecordHotkey::CapsLock,
            ),
            Some(ShortcutAction::Hud(HudShortcutAction::ResetPosition))
        );
        assert_eq!(
            classify_shortcut(
                VK_SPACE,
                EvtKind::KeyDown,
                WinModifiers::ctrl_shift(),
                RecordHotkey::CapsLock,
            ),
            Some(ShortcutAction::Hud(
                HudShortcutAction::ToggleMessagePolishMode
            ))
        );
    }

    #[test]
    fn ctrl_left_alt_r_fires_retry_last_from_audio() {
        assert_eq!(
            classify_shortcut(
                VK_R,
                EvtKind::KeyDown,
                WinModifiers::ctrl_left_alt(),
                RecordHotkey::CapsLock,
            ),
            Some(ShortcutAction::Hud(HudShortcutAction::RetryLastFromAudio))
        );
        // Ctrl+Shift+R must NOT fire retry (that's the browser hard-reload combo).
        assert_eq!(
            classify_shortcut(
                VK_R,
                EvtKind::KeyDown,
                WinModifiers::ctrl_shift(),
                RecordHotkey::CapsLock,
            ),
            None
        );
    }

    #[test]
    fn hud_shortcuts_require_ctrl_shift_without_alt() {
        assert_eq!(
            classify_shortcut(
                VK_OEM_2,
                EvtKind::KeyDown,
                WinModifiers {
                    alt: true,
                    left_alt: true,
                    ..WinModifiers::ctrl_shift()
                },
                RecordHotkey::CapsLock,
            ),
            None
        );
        assert_eq!(
            classify_shortcut(
                VK_OEM_2,
                EvtKind::KeyDown,
                WinModifiers {
                    ctrl: true,
                    ..WinModifiers::default()
                },
                RecordHotkey::CapsLock,
            ),
            None
        );
    }

    #[test]
    fn long_dictation_caps_space_requires_record_hold_and_no_modifiers() {
        assert!(classify_long_dictation(
            VK_SPACE,
            EvtKind::KeyDown,
            WinModifiers::none(),
            RecordHotkey::CapsLock,
            true,
        ));
        assert!(!classify_long_dictation(
            VK_SPACE,
            EvtKind::KeyDown,
            WinModifiers::none(),
            RecordHotkey::CapsLock,
            false,
        ));
        assert!(!classify_long_dictation(
            VK_SPACE,
            EvtKind::KeyDown,
            WinModifiers {
                ctrl: true,
                ..WinModifiers::default()
            },
            RecordHotkey::CapsLock,
            true,
        ));
    }

    #[test]
    fn long_dictation_right_alt_space_is_supported_without_altgr_chords() {
        assert!(classify_long_dictation(
            VK_SPACE,
            EvtKind::KeyDown,
            WinModifiers {
                alt: true,
                right_alt: true,
                ..WinModifiers::default()
            },
            RecordHotkey::RightOption,
            true,
        ));
        assert!(!classify_long_dictation(
            VK_SPACE,
            EvtKind::KeyDown,
            WinModifiers {
                ctrl: true,
                alt: true,
                right_alt: true,
                ..WinModifiers::default()
            },
            RecordHotkey::RightOption,
            true,
        ));
        assert!(!classify_long_dictation(
            VK_SPACE,
            EvtKind::KeyDown,
            WinModifiers::none(),
            RecordHotkey::Function,
            true,
        ));
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
