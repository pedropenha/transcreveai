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
//! (`feed(event, now) -> Vec<HotkeyAction>`): `now` drives the
//! timing-based rules — the FR-002-06 arming window, the FR-002-07
//! double-tap windows, and tap classification — so tests feed the matcher
//! canned event sequences with explicit timestamps.
//!
//! Suppression is *not* decided here: blocking must happen synchronously
//! inside the hook callback, so it is driven by the `BlockingHotkeys` set
//! shared with the listener and updated on register/unregister (and only
//! key-bearing hotkeys belong in it — see `handy_keys::do_register`).

use handy_keys::{Hotkey, KeyEvent};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// FR-002-06: how long after a hold's first press a stray non-modifier
/// key still cancels the session. Matches the `Arming` state of the
/// session machine the dispatcher runs.
pub const ARMING_WINDOW: Duration = Duration::from_millis(250);

/// FR-002-07: a hold released within this long counts as a tap.
pub const TAP_MAX: Duration = Duration::from_millis(250);

/// FR-002-07: a second tap must *begin* within this long after the first
/// tap's release for the pair to count as a double-tap.
pub const DOUBLE_TAP_GAP: Duration = Duration::from_millis(350);

/// Action emitted by [`HotkeyMatcher::feed`] for the dispatcher to execute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotkeyAction {
    /// The named binding's hotkey transitioned to pressed.
    Pressed(String),
    /// The named binding's hotkey transitioned to released.
    Released(String),
    /// A held modifier-only binding (`from`) was extended into a strictly
    /// larger combination (`to`) — e.g. `Ctrl+Win` held, `Alt` added, the
    /// `Ctrl+Win+Alt` binding pressing (FR-002-05: the session's mode
    /// becomes the largest combination seen; v1 ships dictate mode only,
    /// command/note modes consume this in v1.1+). Always emitted right
    /// after the `Pressed` of `to`.
    Promoted {
        /// The held binding whose modifiers are a strict subset.
        from: String,
        /// The binding that just pressed with the larger combination.
        to: String,
    },
    /// A second short tap on the same binding finished inside the
    /// double-tap windows — the hands-free gesture (FR-002-07). Emitted
    /// in addition to the tap's usual `Pressed`/`Released` edges; which
    /// bindings and activation modes honor it is the dispatcher's call.
    DoubleTapped(String),
    /// A non-modifier key no registered binding uses was pressed while a
    /// hold was still inside [`ARMING_WINDOW`]: the session must cancel
    /// silently — the key itself is never suppressed, so it reaches the
    /// OS chord it belongs to (FR-002-06). Emitted once per hold session,
    /// once for every binding held at interrupt time.
    ArmingInterrupted(String),
}

/// A registered binding: the parsed hotkey plus its source string (kept
/// for dispatch logging and `handle_shortcut_event`).
struct RegisteredBinding {
    hotkey: Hotkey,
    hotkey_string: String,
}

/// A completed short press+release that may become the first half of a
/// double-tap (FR-002-07).
struct Tap {
    binding_id: String,
    /// When the first tap released; the second tap must *press* by
    /// `released_at + DOUBLE_TAP_GAP`.
    released_at: Instant,
}

/// Pure hotkey matcher: `KeyEvent` stream in, `HotkeyAction`s out.
///
/// No locks, no I/O, no clock reads — the caller supplies `now`. All state
/// lives in the fields below, which is what makes the matcher testable
/// with canned event sequences (rules/rust/testing.md).
#[derive(Default)]
pub struct HotkeyMatcher {
    /// binding id -> registered hotkey. `BTreeMap` keeps `feed` output
    /// deterministic when several bindings match the same event.
    bindings: BTreeMap<String, RegisteredBinding>,
    /// binding id -> when its press edge fired, for bindings currently
    /// held down. `BTreeMap` keeps multi-binding releases/interrupts
    /// deterministic.
    held: BTreeMap<String, Instant>,
    /// When the current hold session started — the press that took `held`
    /// from empty to non-empty. Anchors the FR-002-06 arming window.
    session_started_at: Option<Instant>,
    /// The arming interrupt was already emitted for this hold session;
    /// further stray keys in the same hold just flow to the OS.
    session_interrupted: bool,
    /// The most recent tap still inside the double-tap window.
    last_tap: Option<Tap>,
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
        // never release cleanly under the new definition. A remembered tap
        // under the old definition must not pair with a tap of the new one
        // into a spurious double-tap.
        self.held.remove(binding_id);
        if self
            .last_tap
            .as_ref()
            .is_some_and(|tap| tap.binding_id == binding_id)
        {
            self.last_tap = None;
        }
        self.reset_session_if_idle();
        Ok(previous)
    }

    /// Remove `binding_id`. Returns the hotkey it held so the caller can
    /// drop it from the suppression set; `None` when it was not registered.
    ///
    /// A binding removed while held loses its release edge silently — the
    /// caller should emit it via [`release_binding`](Self::release_binding)
    /// first when the hold is user-visible.
    pub fn unregister(&mut self, binding_id: &str) -> Option<Hotkey> {
        self.held.remove(binding_id);
        if self
            .last_tap
            .as_ref()
            .is_some_and(|tap| tap.binding_id == binding_id)
        {
            self.last_tap = None;
        }
        self.reset_session_if_idle();
        self.bindings.remove(binding_id).map(|reg| reg.hotkey)
    }

    /// If `binding_id` is currently held, clear the hold and emit `Released`.
    /// Used before unregister/re-register so a held binding's release edge
    /// is surfaced instead of silently dropped.
    pub fn release_binding(&mut self, binding_id: &str) -> Option<HotkeyAction> {
        let released = self
            .held
            .remove(binding_id)
            .map(|_| HotkeyAction::Released(binding_id.to_string()));
        self.reset_session_if_idle();
        released
    }

    /// Emit `Released` for every held binding and clear all hold state.
    /// Called when the event stream dies (watchdog respawn): a dead listener
    /// can no longer deliver key-up edges, and without this a push-to-talk
    /// binding would stay logically pressed forever. Session and tap state
    /// reset too: nothing observed before the stream died can be trusted to
    /// pair with what arrives after the respawn.
    pub fn release_all(&mut self) -> Vec<HotkeyAction> {
        // `BTreeMap` order keeps multi-binding releases deterministic.
        let released: Vec<HotkeyAction> = std::mem::take(&mut self.held)
            .into_keys()
            .map(HotkeyAction::Released)
            .collect();
        self.session_started_at = None;
        self.session_interrupted = false;
        self.last_tap = None;
        released
    }

    /// The persisted hotkey string for a binding (for dispatch/logging).
    pub fn hotkey_string(&self, binding_id: &str) -> Option<&str> {
        self.bindings
            .get(binding_id)
            .map(|reg| reg.hotkey_string.as_str())
    }

    /// The parsed hotkey for a binding — the dispatcher needs the shape
    /// (modifier-only? involves Win/Alt?) to decide menu-mask injection.
    pub fn hotkey(&self, binding_id: &str) -> Option<Hotkey> {
        self.bindings.get(binding_id).map(|reg| reg.hotkey)
    }

    /// A hold session ends when the last held binding releases; drop its
    /// arming bookkeeping so the next hold starts a fresh window.
    fn reset_session_if_idle(&mut self) {
        if self.held.is_empty() {
            self.session_started_at = None;
            self.session_interrupted = false;
        }
    }

    /// Feed one raw key event; pure — no I/O, no locks, no clock reads.
    ///
    /// `now` is the event timestamp as observed by the dispatcher; the
    /// arming-window and double-tap rules consume it.
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
    ///
    /// T-021 additions:
    /// - **Prefix promotion** (FR-002-05): a key-down that presses a
    ///   binding while a strictly-smaller modifier-only binding is held
    ///   also emits [`HotkeyAction::Promoted`].
    /// - **Arming interrupt** (FR-002-06): inside [`ARMING_WINDOW`], a
    ///   non-modifier key-down that is no registered binding's key emits
    ///   [`HotkeyAction::ArmingInterrupted`] for every held binding.
    /// - **Double-tap** (FR-002-07): a release ending a [`TAP_MAX`]-short
    ///   hold, whose press followed the same binding's previous tap by at
    ///   most [`DOUBLE_TAP_GAP`], emits [`HotkeyAction::DoubleTapped`].
    pub fn feed(&mut self, event: &KeyEvent, now: Instant) -> Vec<HotkeyAction> {
        let mut actions = Vec::new();

        if event.is_key_down {
            let pressed: Vec<String> = self
                .bindings
                .iter()
                .filter(|(id, reg)| {
                    !self.held.contains_key(*id)
                        && reg.hotkey.modifiers.matches(event.modifiers)
                        && reg.hotkey.key == event.key
                })
                .map(|(id, _)| id.clone())
                .collect();

            if self.held.is_empty() && !pressed.is_empty() {
                // The press that opens a hold session anchors its arming
                // window (FR-002-06 measures from `Arming`, which the
                // session machine enters on this first edge).
                self.session_started_at = Some(now);
                self.session_interrupted = false;
            }

            for id in pressed {
                let new_modifiers = self.bindings[&id].hotkey.modifiers;
                actions.push(HotkeyAction::Pressed(id.clone()));
                // FR-002-05: every already-held modifier-only binding whose
                // required modifiers are a strict subset of the new
                // binding's is a prefix of it — the session's mode
                // promotes to the larger combination.
                for held_id in self.held.keys() {
                    let held_hotkey = &self.bindings[held_id].hotkey;
                    if held_hotkey.key.is_none()
                        && held_hotkey.modifiers != new_modifiers
                        && held_hotkey.modifiers.bits() & new_modifiers.bits()
                            == held_hotkey.modifiers.bits()
                    {
                        actions.push(HotkeyAction::Promoted {
                            from: held_id.clone(),
                            to: id.clone(),
                        });
                    }
                }
                self.held.insert(id, now);
            }

            // FR-002-06: inside the arming window, a non-modifier key that
            // is no registered binding's key cancels the session — the
            // user is reaching for an OS chord that shares our modifiers
            // (`Ctrl+Win+→`), not dictating. Keys that *are* part of a
            // binding never interrupt, and neither do modifier edges.
            if event.key.is_some()
                && !self.session_interrupted
                && self
                    .session_started_at
                    .is_some_and(|started| now.saturating_duration_since(started) < ARMING_WINDOW)
                && !self
                    .bindings
                    .values()
                    .any(|reg| reg.hotkey.key == event.key)
            {
                self.session_interrupted = true;
                for id in self.held.keys() {
                    actions.push(HotkeyAction::ArmingInterrupted(id.clone()));
                }
            }
        } else {
            let released: Vec<String> = self
                .bindings
                .iter()
                .filter(|(id, reg)| {
                    self.held.contains_key(*id)
                        && ((event.key.is_some() && reg.hotkey.key == event.key)
                            || (event.key.is_none()
                                && !reg.hotkey.modifiers.matches(event.modifiers)))
                })
                .map(|(id, _)| id.clone())
                .collect();
            for id in released {
                if let Some(pressed_at) = self.held.remove(&id) {
                    actions.push(HotkeyAction::Released(id.clone()));
                    // FR-002-07: a short hold is a tap; two taps on the
                    // same binding whose presses are ≤ DOUBLE_TAP_GAP
                    // apart (first release → second press) are a
                    // double-tap.
                    if now.saturating_duration_since(pressed_at) < TAP_MAX {
                        match self.last_tap.take() {
                            Some(tap)
                                if tap.binding_id == id
                                    && pressed_at.saturating_duration_since(tap.released_at)
                                        <= DOUBLE_TAP_GAP =>
                            {
                                actions.push(HotkeyAction::DoubleTapped(id.clone()));
                            }
                            _ => {
                                self.last_tap = Some(Tap {
                                    binding_id: id,
                                    released_at: now,
                                });
                            }
                        }
                    }
                }
            }
            self.reset_session_if_idle();
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
}
