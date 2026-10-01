use super::*;
use std::time::{Duration, Instant};

#[test]
fn press_fires_once_and_release_on_key_up() {
    let mut m = HotkeyMatcher::new();
    register(&mut m, "dictate", "ctrl+win+space");

    let down = key_event(
        Modifiers::CTRL_LEFT | Modifiers::CMD_LEFT,
        Some(Key::Space),
        true,
    );
    assert_eq!(
        m.feed(&down, now()),
        vec![HotkeyAction::Pressed("dictate".into())]
    );
    // Key auto-repeat while held must not re-fire.
    assert!(m.feed(&down, now()).is_empty());

    let up = key_event(
        Modifiers::CTRL_LEFT | Modifiers::CMD_LEFT,
        Some(Key::Space),
        false,
    );
    assert_eq!(
        m.feed(&up, now()),
        vec![HotkeyAction::Released("dictate".into())]
    );
    // A second release fires nothing.
    assert!(m.feed(&up, now()).is_empty());
}

#[test]
fn modifier_only_hotkey_press_and_release() {
    let mut m = HotkeyMatcher::new();
    register(&mut m, "dictate", "ctrl+win");

    // Ctrl down alone: partial match, nothing yet.
    let ctrl_down = modifier_event(Modifiers::CTRL_LEFT, true, Modifiers::CTRL_LEFT);
    assert!(m.feed(&ctrl_down, now()).is_empty());
    // Win down completes the combo.
    let win_down = modifier_event(
        Modifiers::CTRL_LEFT | Modifiers::CMD_LEFT,
        true,
        Modifiers::CMD_LEFT,
    );
    assert_eq!(
        m.feed(&win_down, now()),
        vec![HotkeyAction::Pressed("dictate".into())]
    );
    // Releasing either leg ends the hold (FR-002-11 basis).
    let ctrl_up = modifier_event(Modifiers::CMD_LEFT, false, Modifiers::CTRL_LEFT);
    assert_eq!(
        m.feed(&ctrl_up, now()),
        vec![HotkeyAction::Released("dictate".into())]
    );
}

#[test]
fn normalization_either_side_matches_compound() {
    // LCtrl/RCtrl and LWin/RWin normalize to the compound modifiers:
    // the pattern "ctrl+win" must match whichever side is pressed.
    for mods in [
        Modifiers::CTRL_LEFT | Modifiers::CMD_LEFT,
        Modifiers::CTRL_RIGHT | Modifiers::CMD_RIGHT,
        Modifiers::CTRL_LEFT | Modifiers::CMD_RIGHT,
        Modifiers::CTRL_LEFT | Modifiers::CTRL_RIGHT | Modifiers::CMD_LEFT,
    ] {
        let mut m = HotkeyMatcher::new();
        register(&mut m, "dictate", "ctrl+win");
        let event = modifier_event(mods, true, mods);
        assert_eq!(
            m.feed(&event, now()),
            vec![HotkeyAction::Pressed("dictate".into())],
            "modifier set {:?} should match ctrl+win",
            mods
        );
    }
}

#[test]
fn side_specific_binding_rejects_wrong_side() {
    let mut m = HotkeyMatcher::new();
    register(&mut m, "dictate", "ctrl_left+win");

    let wrong_side = modifier_event(
        Modifiers::CTRL_RIGHT | Modifiers::CMD_LEFT,
        true,
        Modifiers::CTRL_RIGHT,
    );
    assert!(m.feed(&wrong_side, now()).is_empty());

    let right_side = modifier_event(
        Modifiers::CTRL_LEFT | Modifiers::CMD_LEFT,
        true,
        Modifiers::CTRL_LEFT,
    );
    assert_eq!(
        m.feed(&right_side, now()),
        vec![HotkeyAction::Pressed("dictate".into())]
    );
}

#[test]
fn unrelated_key_events_do_not_release_modifier_hotkey() {
    // FR-002-06: a stray non-modifier key inside the arming window
    // interrupts the session, but the hold itself is undisturbed — the
    // release edge still fires when the combo comes up.
    let mut m = HotkeyMatcher::new();
    register(&mut m, "dictate", "ctrl+win");

    let combo = Modifiers::CTRL_LEFT | Modifiers::CMD_LEFT;
    let t0 = Instant::now();
    let win_down = modifier_event(combo, true, Modifiers::CMD_LEFT);
    assert_eq!(
        m.feed(&win_down, t0),
        vec![HotkeyAction::Pressed("dictate".into())]
    );

    let arrow_down = key_event(combo, Some(Key::RightArrow), true);
    assert_eq!(
        m.feed(&arrow_down, t0 + Duration::from_millis(120)),
        vec![HotkeyAction::ArmingInterrupted("dictate".into())]
    );
    let arrow_up = key_event(combo, Some(Key::RightArrow), false);
    assert!(m
        .feed(&arrow_up, t0 + Duration::from_millis(140))
        .is_empty());

    // Still held: releasing Win fires the release.
    let win_up = modifier_event(Modifiers::CTRL_LEFT, false, Modifiers::CMD_LEFT);
    assert_eq!(
        m.feed(&win_up, t0 + Duration::from_millis(160)),
        vec![HotkeyAction::Released("dictate".into())]
    );
}

#[test]
fn modifier_only_hotkey_survives_unrelated_modifier_edge() {
    let mut m = HotkeyMatcher::new();
    register(&mut m, "dictate", "win");

    let win_down = modifier_event(Modifiers::CMD_LEFT, true, Modifiers::CMD_LEFT);
    assert_eq!(
        m.feed(&win_down, now()),
        vec![HotkeyAction::Pressed("dictate".into())]
    );

    // Shift down+up while Win held: extra modifier must not release it.
    let shift_down = modifier_event(
        Modifiers::CMD_LEFT | Modifiers::SHIFT_LEFT,
        true,
        Modifiers::SHIFT_LEFT,
    );
    assert!(m.feed(&shift_down, now()).is_empty());
    let shift_up = modifier_event(Modifiers::CMD_LEFT, false, Modifiers::SHIFT_LEFT);
    assert!(m.feed(&shift_up, now()).is_empty());

    let win_up = modifier_event(Modifiers::empty(), false, Modifiers::CMD_LEFT);
    assert_eq!(
        m.feed(&win_up, now()),
        vec![HotkeyAction::Released("dictate".into())]
    );
}

#[test]
fn key_hotkey_releases_on_modifier_release_while_key_held() {
    // Releasing any leg of the combo ends the hold even when the main
    // key is still down (push-to-talk semantics, FR-002-11).
    let mut m = HotkeyMatcher::new();
    register(&mut m, "dictate", "ctrl+k");

    let down = key_event(Modifiers::CTRL_LEFT, Some(Key::K), true);
    assert_eq!(
        m.feed(&down, now()),
        vec![HotkeyAction::Pressed("dictate".into())]
    );

    let ctrl_up = modifier_event(Modifiers::empty(), false, Modifiers::CTRL_LEFT);
    assert_eq!(
        m.feed(&ctrl_up, now()),
        vec![HotkeyAction::Released("dictate".into())]
    );
}

#[test]
fn key_only_hotkey_requires_no_extra_modifiers() {
    let mut m = HotkeyMatcher::new();
    register(&mut m, "cancel", "escape");

    let with_mod = key_event(Modifiers::SHIFT_LEFT, Some(Key::Escape), true);
    assert!(m.feed(&with_mod, now()).is_empty());

    let plain = key_event(Modifiers::empty(), Some(Key::Escape), true);
    assert_eq!(
        m.feed(&plain, now()),
        vec![HotkeyAction::Pressed("cancel".into())]
    );
}

#[test]
fn sibling_bindings_match_independently() {
    let mut m = HotkeyMatcher::new();
    register(&mut m, "dictate", "ctrl+win");
    register(&mut m, "command", "ctrl+win+alt");

    // Ctrl+Win alone matches only the prefix binding.
    let combo = modifier_event(
        Modifiers::CTRL_LEFT | Modifiers::CMD_LEFT,
        true,
        Modifiers::CMD_LEFT,
    );
    assert_eq!(
        m.feed(&combo, now()),
        vec![HotkeyAction::Pressed("dictate".into())]
    );

    // Adding Alt: the prefix stays held and the longer combo presses —
    // and because `dictate` is a modifier-only strict prefix of
    // `command`, the press also reports the FR-002-05 mode promotion.
    let alt_down = modifier_event(
        Modifiers::CTRL_LEFT | Modifiers::CMD_LEFT | Modifiers::OPT_LEFT,
        true,
        Modifiers::OPT_LEFT,
    );
    assert_eq!(
        m.feed(&alt_down, now()),
        vec![
            HotkeyAction::Pressed("command".into()),
            HotkeyAction::Promoted {
                from: "dictate".into(),
                to: "command".into()
            }
        ]
    );
}

#[test]
fn duplicate_hotkey_across_bindings_is_rejected() {
    let mut m = HotkeyMatcher::new();
    register(&mut m, "a", "ctrl+win");
    let hotkey: Hotkey = "ctrl+win".parse().expect("parse");
    let err = m
        .register("b", hotkey, "ctrl+win".to_string())
        .expect_err("duplicate must be rejected");
    assert!(
        err.contains("binding 'a'"),
        "error should name the holder: {err}"
    );
}

#[test]
fn unregister_stops_events_and_returns_hotkey() {
    let mut m = HotkeyMatcher::new();
    register(&mut m, "dictate", "ctrl+win");

    let removed = m.unregister("dictate");
    assert_eq!(removed, Some("ctrl+win".parse().expect("parse")));
    assert!(m.unregister("dictate").is_none());

    let win_down = modifier_event(
        Modifiers::CTRL_LEFT | Modifiers::CMD_LEFT,
        true,
        Modifiers::CMD_LEFT,
    );
    assert!(m.feed(&win_down, now()).is_empty());
}

#[test]
fn reregister_replaces_and_reports_previous_hotkey() {
    let mut m = HotkeyMatcher::new();
    register(&mut m, "dictate", "ctrl+win");

    let new_hotkey: Hotkey = "alt+m".parse().expect("parse");
    let previous = m
        .register("dictate", new_hotkey, "alt+m".to_string())
        .expect("re-register same binding");
    assert_eq!(previous, Some("ctrl+win".parse().expect("parse")));

    let alt_m = key_event(Modifiers::OPT_LEFT, Some(Key::M), true);
    assert_eq!(
        m.feed(&alt_m, now()),
        vec![HotkeyAction::Pressed("dictate".into())]
    );
}

#[test]
fn release_before_press_emits_nothing() {
    let mut m = HotkeyMatcher::new();
    register(&mut m, "dictate", "ctrl+k");
    let up = key_event(Modifiers::CTRL_LEFT, Some(Key::K), false);
    assert!(m.feed(&up, now()).is_empty());
}

#[test]
fn release_all_emits_released_for_every_held_binding() {
    // Mirrors the watchdog path: the event stream dies mid-hold, so the
    // matcher must synthesize the release edges itself.
    let mut m = HotkeyMatcher::new();
    register(&mut m, "dictate", "ctrl+win");
    register(&mut m, "command", "ctrl+win+alt");
    register(&mut m, "cancel", "escape");

    let combo = Modifiers::CTRL_LEFT | Modifiers::CMD_LEFT;
    let win_down = modifier_event(combo, true, Modifiers::CMD_LEFT);
    assert_eq!(
        m.feed(&win_down, now()),
        vec![HotkeyAction::Pressed("dictate".into())]
    );
    let alt_down = modifier_event(combo | Modifiers::OPT_LEFT, true, Modifiers::OPT_LEFT);
    assert_eq!(
        m.feed(&alt_down, now()),
        vec![
            HotkeyAction::Pressed("command".into()),
            HotkeyAction::Promoted {
                from: "dictate".into(),
                to: "command".into()
            }
        ]
    );

    // "cancel" was never pressed: only the two held bindings release.
    assert_eq!(
        m.release_all(),
        vec![
            HotkeyAction::Released("command".into()),
            HotkeyAction::Released("dictate".into())
        ]
    );
    assert!(m.release_all().is_empty());

    // Late key-up edges for the dead stream emit nothing — the hold is gone.
    let win_up = modifier_event(Modifiers::CTRL_LEFT, false, Modifiers::CMD_LEFT);
    assert!(m.feed(&win_up, now()).is_empty());
}

#[test]
fn release_binding_emits_only_for_held_binding() {
    let mut m = HotkeyMatcher::new();
    register(&mut m, "dictate", "ctrl+win");
    register(&mut m, "command", "alt+m");

    assert!(m.release_binding("dictate").is_none());
    assert!(m.release_binding("unknown").is_none());

    let win_down = modifier_event(
        Modifiers::CTRL_LEFT | Modifiers::CMD_LEFT,
        true,
        Modifiers::CMD_LEFT,
    );
    assert_eq!(
        m.feed(&win_down, now()),
        vec![HotkeyAction::Pressed("dictate".into())]
    );

    assert_eq!(
        m.release_binding("dictate"),
        Some(HotkeyAction::Released("dictate".into()))
    );
    assert!(m.release_binding("dictate").is_none());
}
