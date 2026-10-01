use super::*;
use std::time::{Duration, Instant};

// ------------------------------------------------------------------
// FR-002-06 — arming-window interrupt (AC-002-06)
// ------------------------------------------------------------------

/// The `Ctrl+Win+→` case: the stray arrow interrupts the arming
/// session once; the key itself is never in the suppression set, so
/// it reaches the OS (desktop switch happens shell-side).
#[test]
fn stray_key_inside_arming_window_interrupts_once() {
    let mut m = HotkeyMatcher::new();
    register(&mut m, "dictate", "ctrl+win");

    let t0 = Instant::now();
    let combo = Modifiers::CTRL_LEFT | Modifiers::CMD_LEFT;
    m.feed(&modifier_event(combo, true, Modifiers::CMD_LEFT), t0);

    let arrow_down = key_event(combo, Some(Key::RightArrow), true);
    assert_eq!(
        m.feed(&arrow_down, t0 + Duration::from_millis(100)),
        vec![HotkeyAction::ArmingInterrupted("dictate".into())]
    );
    // A second stray key in the same hold does not re-interrupt.
    let another_down = key_event(combo, Some(Key::RightArrow), true);
    assert!(m
        .feed(&another_down, t0 + Duration::from_millis(150))
        .is_empty());
}

/// Past the arming window a stray key is just a stray key — a
/// recording in progress is not cancelled by an unrelated chord.
#[test]
fn stray_key_after_arming_window_does_not_interrupt() {
    let mut m = HotkeyMatcher::new();
    register(&mut m, "dictate", "ctrl+win");

    let t0 = Instant::now();
    let combo = Modifiers::CTRL_LEFT | Modifiers::CMD_LEFT;
    m.feed(&modifier_event(combo, true, Modifiers::CMD_LEFT), t0);

    let arrow_down = key_event(combo, Some(Key::RightArrow), true);
    assert!(m
        .feed(&arrow_down, t0 + ARMING_WINDOW + Duration::from_millis(1))
        .is_empty());
}

/// "não faça parte de nenhum atalho": a key that is some registered
/// binding's key never interrupts, even pressed with the wrong
/// modifiers.
#[test]
fn key_of_a_registered_binding_never_interrupts() {
    let mut m = HotkeyMatcher::new();
    register(&mut m, "dictate", "ctrl+win");
    register(&mut m, "paste_last", "alt+shift+v");

    let t0 = Instant::now();
    let combo = Modifiers::CTRL_LEFT | Modifiers::CMD_LEFT;
    m.feed(&modifier_event(combo, true, Modifiers::CMD_LEFT), t0);

    let v_down = key_event(combo, Some(Key::V), true);
    assert!(m.feed(&v_down, t0 + Duration::from_millis(100)).is_empty());
}

/// Only non-modifier keys interrupt — a modifier edge inside the
/// window is how prefix promotion happens, not a cancel.
#[test]
fn modifier_edges_never_interrupt_arming() {
    let mut m = HotkeyMatcher::new();
    register(&mut m, "dictate", "ctrl+win");

    let t0 = Instant::now();
    m.feed(
        &modifier_event(
            Modifiers::CTRL_LEFT | Modifiers::CMD_LEFT,
            true,
            Modifiers::CMD_LEFT,
        ),
        t0,
    );

    // An unregistered modifier join mid-arming: no press, no interrupt.
    let shift_down = modifier_event(
        Modifiers::CTRL_LEFT | Modifiers::CMD_LEFT | Modifiers::SHIFT_LEFT,
        true,
        Modifiers::SHIFT_LEFT,
    );
    assert!(m
        .feed(&shift_down, t0 + Duration::from_millis(100))
        .is_empty());
}

/// The arming window anchors to the *session's* first press: a stray
/// key after a mid-window promotion still interrupts every binding
/// the hold is running.
#[test]
fn arming_interrupt_covers_every_held_binding() {
    let mut m = HotkeyMatcher::new();
    register(&mut m, "dictate", "ctrl+win");
    register(&mut m, "command", "ctrl+win+alt");

    let t0 = Instant::now();
    let base = Modifiers::CTRL_LEFT | Modifiers::CMD_LEFT;
    m.feed(&modifier_event(base, true, Modifiers::CMD_LEFT), t0);
    m.feed(
        &modifier_event(base | Modifiers::OPT_LEFT, true, Modifiers::OPT_LEFT),
        t0 + Duration::from_millis(150),
    );

    let arrow_down = key_event(base | Modifiers::OPT_LEFT, Some(Key::RightArrow), true);
    assert_eq!(
        m.feed(&arrow_down, t0 + Duration::from_millis(200)),
        vec![
            HotkeyAction::ArmingInterrupted("command".into()),
            HotkeyAction::ArmingInterrupted("dictate".into())
        ]
    );
}

/// A fresh hold gets a fresh window: after a full release the next
/// press re-arms the interrupt.
#[test]
fn next_hold_rearms_the_interrupt() {
    let mut m = HotkeyMatcher::new();
    register(&mut m, "dictate", "ctrl+win");

    let t0 = Instant::now();
    let combo = Modifiers::CTRL_LEFT | Modifiers::CMD_LEFT;
    m.feed(&modifier_event(combo, true, Modifiers::CMD_LEFT), t0);
    m.feed(
        &key_event(combo, Some(Key::RightArrow), true),
        t0 + Duration::from_millis(100),
    );
    m.feed(
        &modifier_event(Modifiers::CTRL_LEFT, false, Modifiers::CMD_LEFT),
        t0 + Duration::from_millis(150),
    );

    m.feed(
        &modifier_event(combo, true, Modifiers::CMD_LEFT),
        t0 + Duration::from_millis(500),
    );
    let arrow_down = key_event(combo, Some(Key::RightArrow), true);
    assert_eq!(
        m.feed(&arrow_down, t0 + Duration::from_millis(550)),
        vec![HotkeyAction::ArmingInterrupted("dictate".into())]
    );
}
