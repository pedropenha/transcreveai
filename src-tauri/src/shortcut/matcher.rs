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
mod tests;
