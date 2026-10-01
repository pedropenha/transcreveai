use super::*;
use std::time::{Duration, Instant};

// ------------------------------------------------------------------
// FR-002-05 — prefix promotion (AC-002-08 groundwork)
// ------------------------------------------------------------------

/// Ctrl+Win held, Alt joins at +150 ms: the `ctrl+win+alt` binding
/// presses and reports the promotion of the held prefix.
#[test]
fn prefix_promotion_reports_largest_combination() {
    let mut m = HotkeyMatcher::new();
    register(&mut m, "dictate", "ctrl+win");
    register(&mut m, "command", "ctrl+win+alt");

    let t0 = Instant::now();
    let win_down = modifier_event(
        Modifiers::CTRL_LEFT | Modifiers::CMD_LEFT,
        true,
        Modifiers::CMD_LEFT,
    );
    assert_eq!(
        m.feed(&win_down, t0),
        vec![HotkeyAction::Pressed("dictate".into())]
    );

    let alt_down = modifier_event(
        Modifiers::CTRL_LEFT | Modifiers::CMD_LEFT | Modifiers::OPT_LEFT,
        true,
        Modifiers::OPT_LEFT,
    );
    assert_eq!(
        m.feed(&alt_down, t0 + Duration::from_millis(150)),
        vec![
            HotkeyAction::Pressed("command".into()),
            HotkeyAction::Promoted {
                from: "dictate".into(),
                to: "command".into()
            }
        ]
    );

    // Releasing Alt demotes the combo back — raw edges only; whether
    // the session's mode sticks to the largest combination seen is the
    // session machine's call (FR-002-05).
    let alt_up = modifier_event(
        Modifiers::CTRL_LEFT | Modifiers::CMD_LEFT,
        false,
        Modifiers::OPT_LEFT,
    );
    assert_eq!(
        m.feed(&alt_up, t0 + Duration::from_millis(200)),
        vec![HotkeyAction::Released("command".into())]
    );
    let win_up = modifier_event(Modifiers::CTRL_LEFT, false, Modifiers::CMD_LEFT);
    assert_eq!(
        m.feed(&win_up, t0 + Duration::from_millis(250)),
        vec![HotkeyAction::Released("dictate".into())]
    );
}

/// Every held strict prefix promotes the extending press — the
/// session's mode is the largest combination seen.
#[test]
fn prefix_promotion_fires_for_every_held_prefix() {
    let mut m = HotkeyMatcher::new();
    register(&mut m, "dictate", "ctrl+win");
    register(&mut m, "command", "ctrl+win+alt");
    register(&mut m, "note", "ctrl+win+alt+shift");

    let t0 = Instant::now();
    let base = Modifiers::CTRL_LEFT | Modifiers::CMD_LEFT;
    m.feed(&modifier_event(base, true, Modifiers::CMD_LEFT), t0);
    m.feed(
        &modifier_event(base | Modifiers::OPT_LEFT, true, Modifiers::OPT_LEFT),
        t0 + Duration::from_millis(50),
    );

    let shift_down = modifier_event(
        base | Modifiers::OPT_LEFT | Modifiers::SHIFT_LEFT,
        true,
        Modifiers::SHIFT_LEFT,
    );
    assert_eq!(
        m.feed(&shift_down, t0 + Duration::from_millis(100)),
        vec![
            HotkeyAction::Pressed("note".into()),
            HotkeyAction::Promoted {
                from: "command".into(),
                to: "note".into()
            },
            HotkeyAction::Promoted {
                from: "dictate".into(),
                to: "note".into()
            }
        ]
    );
}

/// A key-bearing binding sharing the held combo's modifiers is a
/// different hotkey, not a mode extension — no promotion.
#[test]
fn same_modifiers_plus_key_is_not_a_promotion() {
    let mut m = HotkeyMatcher::new();
    register(&mut m, "dictate", "ctrl+win");
    register(&mut m, "handsfree", "ctrl+win+space");

    let t0 = Instant::now();
    let combo = Modifiers::CTRL_LEFT | Modifiers::CMD_LEFT;
    m.feed(&modifier_event(combo, true, Modifiers::CMD_LEFT), t0);

    let space_down = key_event(combo, Some(Key::Space), true);
    assert_eq!(
        m.feed(&space_down, t0 + Duration::from_millis(80)),
        vec![HotkeyAction::Pressed("handsfree".into())]
    );
}

/// A held binding that itself carries a key never counts as a prefix.
#[test]
fn key_bearing_held_binding_is_never_a_prefix() {
    let mut m = HotkeyMatcher::new();
    register(&mut m, "dictate", "ctrl+k");
    register(&mut m, "command", "ctrl+win+alt");

    let t0 = Instant::now();
    let k_down = key_event(Modifiers::CTRL_LEFT, Some(Key::K), true);
    m.feed(&k_down, t0);

    // Ctrl held; Win+Alt join -> `command` presses, but `ctrl+k` is a
    // keyed combo: its hold is not a mode prefix.
    let alt_down = modifier_event(
        Modifiers::CTRL_LEFT | Modifiers::CMD_LEFT | Modifiers::OPT_LEFT,
        true,
        Modifiers::OPT_LEFT,
    );
    assert_eq!(
        m.feed(&alt_down, t0 + Duration::from_millis(50)),
        vec![HotkeyAction::Pressed("command".into())]
    );
}
