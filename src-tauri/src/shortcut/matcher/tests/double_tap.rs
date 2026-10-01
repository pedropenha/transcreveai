use super::*;
use std::time::{Duration, Instant};

// ------------------------------------------------------------------
// FR-002-07 — double-tap hands-free (AC-002-03)
// ------------------------------------------------------------------

/// Two short taps (< 250 ms each) with the second press ≤ 350 ms
/// after the first tap's release emit `DoubleTapped` on the second
/// tap's release — in addition to the usual press/release edges.
#[test]
fn double_tap_fires_on_second_tap() {
    let mut m = HotkeyMatcher::new();
    register(&mut m, "dictate", "ctrl+win");

    let t0 = Instant::now();
    let combo = Modifiers::CTRL_LEFT | Modifiers::CMD_LEFT;
    let down = modifier_event(combo, true, Modifiers::CMD_LEFT);
    let up = modifier_event(Modifiers::CTRL_LEFT, false, Modifiers::CMD_LEFT);

    assert_eq!(
        m.feed(&down, t0),
        vec![HotkeyAction::Pressed("dictate".into())]
    );
    assert_eq!(
        m.feed(&up, t0 + Duration::from_millis(100)),
        vec![HotkeyAction::Released("dictate".into())]
    );
    assert_eq!(
        m.feed(&down, t0 + Duration::from_millis(300)),
        vec![HotkeyAction::Pressed("dictate".into())]
    );
    assert_eq!(
        m.feed(&up, t0 + Duration::from_millis(400)),
        vec![
            HotkeyAction::Released("dictate".into()),
            HotkeyAction::DoubleTapped("dictate".into())
        ]
    );
}

/// Both taps must be short: a second press held ≥ 250 ms is a normal
/// push-to-talk hold, not a double-tap.
#[test]
fn long_second_hold_is_not_a_double_tap() {
    let mut m = HotkeyMatcher::new();
    register(&mut m, "dictate", "ctrl+win");

    let t0 = Instant::now();
    let combo = Modifiers::CTRL_LEFT | Modifiers::CMD_LEFT;
    let down = modifier_event(combo, true, Modifiers::CMD_LEFT);
    let up = modifier_event(Modifiers::CTRL_LEFT, false, Modifiers::CMD_LEFT);

    m.feed(&down, t0);
    m.feed(&up, t0 + Duration::from_millis(100));
    m.feed(&down, t0 + Duration::from_millis(200));
    assert_eq!(
        m.feed(&up, t0 + Duration::from_millis(200) + TAP_MAX),
        vec![HotkeyAction::Released("dictate".into())]
    );
}

/// The second tap must press within `DOUBLE_TAP_GAP` of the first
/// tap's release — later pairs start a new count instead.
#[test]
fn double_tap_gap_expires() {
    let mut m = HotkeyMatcher::new();
    register(&mut m, "dictate", "ctrl+win");

    let t0 = Instant::now();
    let combo = Modifiers::CTRL_LEFT | Modifiers::CMD_LEFT;
    let down = modifier_event(combo, true, Modifiers::CMD_LEFT);
    let up = modifier_event(Modifiers::CTRL_LEFT, false, Modifiers::CMD_LEFT);

    m.feed(&down, t0);
    m.feed(&up, t0 + Duration::from_millis(100));
    // Second press lands 400 ms after the first tap's release.
    m.feed(&down, t0 + Duration::from_millis(500));
    assert_eq!(
        m.feed(&up, t0 + Duration::from_millis(600)),
        vec![HotkeyAction::Released("dictate".into())],
        "no double-tap once the gap window expired"
    );
    // That late tap is itself a first tap for the next window.
    m.feed(&down, t0 + Duration::from_millis(800));
    assert_eq!(
        m.feed(&up, t0 + Duration::from_millis(900)),
        vec![
            HotkeyAction::Released("dictate".into()),
            HotkeyAction::DoubleTapped("dictate".into())
        ]
    );
}

/// The two taps must hit the same binding.
#[test]
fn double_tap_requires_same_binding() {
    let mut m = HotkeyMatcher::new();
    register(&mut m, "dictate", "ctrl+win");
    register(&mut m, "post_process", "ctrl+shift");

    let t0 = Instant::now();
    let dictate_down = modifier_event(
        Modifiers::CTRL_LEFT | Modifiers::CMD_LEFT,
        true,
        Modifiers::CMD_LEFT,
    );
    let dictate_up = modifier_event(Modifiers::CTRL_LEFT, false, Modifiers::CMD_LEFT);
    let pp_down = modifier_event(
        Modifiers::CTRL_LEFT | Modifiers::SHIFT_LEFT,
        true,
        Modifiers::SHIFT_LEFT,
    );
    let pp_up = modifier_event(Modifiers::CTRL_LEFT, false, Modifiers::SHIFT_LEFT);

    m.feed(&dictate_down, t0);
    m.feed(&dictate_up, t0 + Duration::from_millis(100));
    m.feed(&pp_down, t0 + Duration::from_millis(200));
    assert_eq!(
        m.feed(&pp_up, t0 + Duration::from_millis(300)),
        vec![HotkeyAction::Released("post_process".into())],
        "different bindings do not pair into a double-tap"
    );
}

/// A single tap followed by silence emits nothing extra.
#[test]
fn single_tap_emits_no_double_tap() {
    let mut m = HotkeyMatcher::new();
    register(&mut m, "dictate", "ctrl+win");

    let t0 = Instant::now();
    let combo = Modifiers::CTRL_LEFT | Modifiers::CMD_LEFT;
    m.feed(&modifier_event(combo, true, Modifiers::CMD_LEFT), t0);
    assert_eq!(
        m.feed(
            &modifier_event(Modifiers::CTRL_LEFT, false, Modifiers::CMD_LEFT),
            t0 + Duration::from_millis(100)
        ),
        vec![HotkeyAction::Released("dictate".into())]
    );
}

/// A stream death mid-gesture clears the remembered tap: the first
/// tap after `release_all` can never pair with a pre-death one.
#[test]
fn release_all_drops_remembered_tap() {
    let mut m = HotkeyMatcher::new();
    register(&mut m, "dictate", "ctrl+win");

    let t0 = Instant::now();
    let combo = Modifiers::CTRL_LEFT | Modifiers::CMD_LEFT;
    let down = modifier_event(combo, true, Modifiers::CMD_LEFT);
    let up = modifier_event(Modifiers::CTRL_LEFT, false, Modifiers::CMD_LEFT);

    m.feed(&down, t0);
    m.feed(&up, t0 + Duration::from_millis(100));
    assert!(m.release_all().is_empty(), "nothing held between taps");

    m.feed(&down, t0 + Duration::from_millis(200));
    assert_eq!(
        m.feed(&up, t0 + Duration::from_millis(300)),
        vec![HotkeyAction::Released("dictate".into())],
        "a tap must not pair with one from before the stream died"
    );
}
