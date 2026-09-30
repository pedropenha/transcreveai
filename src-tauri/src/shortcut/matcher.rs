//! Pure hotkey matcher fed by raw `KeyEvent`s from the OS hook.
//!
//! Pipeline split (spec F002, NFR-002-01): the handy-keys `KeyboardListener`
//! thread runs the low-level hook whose callback only classifies raw input
//! and ships `KeyEvent`s over a channel. This matcher is the pure,
//! side-effect-free half of the pipeline — it keeps the registered bindings
//! and which of them are currently held, and turns each event into a list
//! of [`HotkeyAction`]s.
//!
//! The signature deliberately mirrors the spec's target
//! (`feed(event, now) -> Vec<HotkeyAction>`): T-021 adds the timing-based
//! rules (double-tap, arming window, prefix promotion) that consume `now`,
//! plus richer actions such as cancel-on-unrelated-key (FR-002-06). For
//! now `now` is accepted but unused, so tests already drive the matcher
//! with explicit timestamps.
//!
//! Suppression is *not* decided here: blocking must happen synchronously
//! inside the hook callback, so it is driven by the `BlockingHotkeys` set
//! shared with the listener and updated on register/unregister.

use handy_keys::{Hotkey, KeyEvent};
use std::collections::{BTreeMap, HashSet};
use std::time::Instant;

/// Action emitted by [`HotkeyMatcher::feed`] for the dispatcher to execute.
///
/// Carries the binding id only; T-021 extends this enum with the
/// session-level actions (double-tap toggle, mode promotion, arming
/// cancel) the richer FR-002 rules need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotkeyAction {
    /// The named binding's hotkey transitioned to pressed.
    Pressed(String),
    /// The named binding's hotkey transitioned to released.
    Released(String),
}

impl HotkeyAction {
    /// The binding id this action refers to.
    pub fn binding_id(&self) -> &str {
        match self {
            HotkeyAction::Pressed(id) | HotkeyAction::Released(id) => id,
        }
    }
}

/// A registered binding: the parsed hotkey plus its source string (kept
/// for dispatch logging and `handle_shortcut_event`).
struct RegisteredBinding {
    hotkey: Hotkey,
    hotkey_string: String,
}

/// Pure hotkey matcher: `KeyEvent` stream in, `HotkeyAction`s out.
///
/// No locks, no I/O, no clock reads — the caller supplies `now`. All state
/// lives in the two maps below, which is what makes the matcher testable
/// with canned event sequences (rules/rust/testing.md).
#[derive(Default)]
pub struct HotkeyMatcher {
    /// binding id -> registered hotkey. `BTreeMap` keeps `feed` output
    /// deterministic when several bindings match the same event.
    bindings: BTreeMap<String, RegisteredBinding>,
    /// binding ids whose hotkey is currently held down.
    held: HashSet<String>,
}

impl HotkeyMatcher {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `binding_id` with `hotkey`.
    ///
    /// `hotkey_string` is the binding's persisted representation, kept for
    /// logging/dispatch. Returns the previously registered hotkey when the
    /// same binding id is re-registered, so the caller can evict it from
    /// the suppression set.
    ///
    /// Errors when `hotkey` is already claimed by a *different* binding —
    /// two bindings fighting over one combo would silently shadow each
    /// other (same contract `handy_keys::HotkeyManager` enforced).
    pub fn register(
        &mut self,
        binding_id: &str,
        hotkey: Hotkey,
        hotkey_string: String,
    ) -> Result<Option<Hotkey>, String> {
        if let Some(other) = self
            .bindings
            .iter()
            .find(|(id, reg)| id.as_str() != binding_id && reg.hotkey == hotkey)
        {
            return Err(format!(
                "Hotkey '{}' is already registered to binding '{}'",
                hotkey_string, other.0
            ));
        }
        let previous = self
            .bindings
            .insert(
                binding_id.to_string(),
                RegisteredBinding {
                    hotkey,
                    hotkey_string,
                },
            )
            .map(|old| old.hotkey);
        // A re-registered binding starts unheld: its previous hold could
        // never release cleanly under the new definition.
        self.held.remove(binding_id);
        Ok(previous)
    }

    /// Remove `binding_id`. Returns the hotkey it held so the caller can
    /// drop it from the suppression set; `None` when it was not registered.
    pub fn unregister(&mut self, binding_id: &str) -> Option<Hotkey> {
        self.held.remove(binding_id);
        self.bindings.remove(binding_id).map(|reg| reg.hotkey)
    }

    /// The persisted hotkey string for a binding (for dispatch/logging).
    pub fn hotkey_string(&self, binding_id: &str) -> Option<&str> {
        self.bindings
            .get(binding_id)
            .map(|reg| reg.hotkey_string.as_str())
    }

    /// Feed one raw key event; pure — no I/O, no locks, no clock reads.
    ///
    /// `_now` is the event timestamp as observed by the dispatcher. The
    /// current press/release rules are edge-triggered and do not need it;
    /// T-021's double-tap and arming-window rules will.
    ///
    /// Semantics (matching the upstream `handy_keys` manager this replaces):
    /// - key-down presses every binding whose `modifiers` pattern matches
    ///   the event's modifier set (`Modifiers::matches` widens
    ///   `LCtrl/RCtrl` -> `Ctrl`, `LWin/RWin` -> `Cmd`) and whose `key`
    ///   equals the event's key — including modifier-only hotkeys
    ///   (`key: None`) on `key: None` events.
    /// - key-up releases a held binding when the released key is the
    ///   hotkey's key, or — for modifier events (`key: None`) — when the
    ///   remaining modifiers no longer match the pattern. Any leg of the
    ///   combo ending the hold is what push-to-talk (FR-002-11) needs.
    pub fn feed(&mut self, event: &KeyEvent, _now: Instant) -> Vec<HotkeyAction> {
        let mut actions = Vec::new();

        if event.is_key_down {
            for (id, reg) in &self.bindings {
                if !self.held.contains(id)
                    && reg.hotkey.modifiers.matches(event.modifiers)
                    && reg.hotkey.key == event.key
                {
                    actions.push(HotkeyAction::Pressed(id.clone()));
                }
            }
            for action in &actions {
                self.held.insert(action.binding_id().to_string());
            }
        } else {
            let released: Vec<String> = self
                .bindings
                .iter()
                .filter(|(id, reg)| {
                    self.held.contains(*id)
                        && ((event.key.is_some() && reg.hotkey.key == event.key)
                            || (event.key.is_none()
                                && !reg.hotkey.modifiers.matches(event.modifiers)))
                })
                .map(|(id, _)| id.clone())
                .collect();
            for id in released {
                self.held.remove(&id);
                actions.push(HotkeyAction::Released(id));
            }
        }

        actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use handy_keys::{Key, Modifiers};

    fn now() -> Instant {
        Instant::now()
    }

    fn key_event(modifiers: Modifiers, key: Option<Key>, is_key_down: bool) -> KeyEvent {
        KeyEvent {
            modifiers,
            key,
            is_key_down,
            changed_modifier: None,
        }
    }

    fn modifier_event(modifiers: Modifiers, is_key_down: bool, changed: Modifiers) -> KeyEvent {
        KeyEvent {
            modifiers,
            key: None,
            is_key_down,
            changed_modifier: Some(changed),
        }
    }

    fn register(matcher: &mut HotkeyMatcher, id: &str, raw: &str) {
        let hotkey: Hotkey = raw.parse().expect("test hotkey must parse");
        matcher
            .register(id, hotkey, raw.to_string())
            .expect("test registration must succeed");
    }

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
        // FR-002-06 groundwork: a stray key while the combo is held must
        // flow through without disturbing the held binding (the session
        // layer — T-021/T-022 — decides to cancel, not the matcher).
        let mut m = HotkeyMatcher::new();
        register(&mut m, "dictate", "ctrl+win");

        let combo = Modifiers::CTRL_LEFT | Modifiers::CMD_LEFT;
        let win_down = modifier_event(combo, true, Modifiers::CMD_LEFT);
        assert_eq!(
            m.feed(&win_down, now()),
            vec![HotkeyAction::Pressed("dictate".into())]
        );

        let arrow_down = key_event(combo, Some(Key::RightArrow), true);
        assert!(m.feed(&arrow_down, now()).is_empty());
        let arrow_up = key_event(combo, Some(Key::RightArrow), false);
        assert!(m.feed(&arrow_up, now()).is_empty());

        // Still held: releasing Win fires the release.
        let win_up = modifier_event(Modifiers::CTRL_LEFT, false, Modifiers::CMD_LEFT);
        assert_eq!(
            m.feed(&win_up, now()),
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

        // Adding Alt: the prefix stays held (promotion is T-021's job —
        // here the matcher only reports raw transitions) and the longer
        // combo now also matches and presses.
        let alt_down = modifier_event(
            Modifiers::CTRL_LEFT | Modifiers::CMD_LEFT | Modifiers::OPT_LEFT,
            true,
            Modifiers::OPT_LEFT,
        );
        assert_eq!(
            m.feed(&alt_down, now()),
            vec![HotkeyAction::Pressed("command".into())]
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
        assert!(err.contains("a"), "error should name the holder: {err}");
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
}
