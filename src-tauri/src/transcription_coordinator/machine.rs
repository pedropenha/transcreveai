//! Pure state machine for dictation sessions (spec F002, "Sessão de ditado").
//!
//! Owns every lifecycle decision — release grace, hold-vs-tap classification,
//! the arming window, double-tap hands-free, debounce, the FIFO queue of
//! pending sessions, the duration limit and its T-60s warning, cancel, and
//! pipeline-phase tracking. Produces [`Effect`]s and [`SessionStateEvent`]s
//! instead of touching the app, so unit tests exercise the exact production
//! transitions without a Tauri `AppHandle` or real timers.
//!
//! States follow FR-002-09 / plan.md §5:
//!
//! ```text
//! Idle → Arming → Recording → Transcribing → Processing → Inserting → Done | Error
//! ```
//!
//! * `Arming`: capture already runs (FR-002-10) but the session is not yet
//!   committed — it is cancelled by a short tap without a second tap
//!   (push-to-talk) or by an unrelated OS-shortcut key (FR-002-06, delivered
//!   as a cancel).
//! * `Recording`: committed capture. Ends on release (push-to-talk), on the
//!   next press (toggle / locked session), or on the duration limit
//!   (FR-002-13), which stops gracefully and processes normally.
//! * `Transcribing`/`Processing`/`Inserting`: the pipeline is busy. Presses
//!   while busy are remembered in a FIFO queue of up to `session_queue_size`
//!   pending sessions (FR-002-16); insertions therefore happen in order.
//! * `Done`/`Error`: short dwells that keep the outcome visible before
//!   returning to `Idle`. A press during the dwell starts a new session.
//!
//! All three activation modes run through one machine. A session starts on
//! key-down in every mode; what differs is how it ends:
//!
//! * push-to-talk — a hold ends on release; a tap waits out the double-tap
//!   window (FR-002-07): a second press locks hands-free, otherwise the
//!   session is discarded in `Arming` (AC-002-02)
//! * toggle — releases are ignored, the next press stops (locked from the
//!   start)
//! * hold-or-toggle — a release after a long hold stops; a release after a
//!   short tap locks the session, and the next press stops

use crate::settings::{AppSettings, ShortcutActivation};
use log::debug;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Event name for session lifecycle updates (contracts.md §5). Every stage
/// transition is emitted on this channel for the Flow Bar / Hub.
pub const SESSION_STATE_EVENT: &str = "session://state";
/// Event name carrying the final result of a session (inserted text).
pub const SESSION_RESULT_EVENT: &str = "session://result";

const DEBOUNCE: Duration = Duration::from_millis(30);
const RELEASE_GRACE: Duration = Duration::from_millis(50);
/// FR-002-06/-09: how long a session may stay uncommitted. A hold that
/// survives past this window becomes `Recording`; a release inside it is a
/// tap (discard or double-tap candidate), not a dictation.
const ARMING_WINDOW: Duration = Duration::from_millis(250);
/// FR-002-07: maximum gap between the two taps that start hands-free.
const DOUBLE_TAP_WINDOW: Duration = Duration::from_millis(350);
/// plan.md §5: `Done → Idle` after 600 ms so completion is visible.
const DONE_DWELL: Duration = Duration::from_millis(600);
/// `Error → Idle` dwell: long enough for the Flow Bar to surface the failure
/// and its "Tentar novamente" affordance (AC-002-09).
const ERROR_DWELL: Duration = Duration::from_secs(3);
/// FR-002-13: the Flow Bar warning fires this long before the duration limit.
const DURATION_WARN_LEAD: Duration = Duration::from_secs(60);
/// Spec limits for `max_dictation_minutes` (FR-002-13): 1–20 minutes.
pub(crate) const MIN_DICTATION_LIMIT_MIN: u64 = 1;
pub(crate) const MAX_DICTATION_LIMIT_MIN: u64 = 20;

/// Per-input policy snapshot taken from settings. Carrying it on the input
/// keeps [`CoordinatorState`] pure — no store reads inside the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionPolicy {
    /// Maximum pending sessions remembered while the pipeline is busy
    /// (`session_queue_size`, FR-002-16).
    pub queue_capacity: usize,
    /// Maximum recording length (`max_dictation_minutes`, FR-002-13).
    pub max_duration: Duration,
    /// Whether a double tap on the push-to-talk shortcut starts hands-free
    /// (FR-002-07).
    pub double_tap: bool,
}

impl SessionPolicy {
    /// Snapshot the session-related settings; clamps enforce the spec ranges
    /// even if the store somehow holds an out-of-range value.
    pub fn from_settings(settings: &AppSettings) -> Self {
        Self {
            queue_capacity: settings.session_queue_size.max(1),
            max_duration: Duration::from_secs(
                settings
                    .max_dictation_minutes
                    .clamp(MIN_DICTATION_LIMIT_MIN, MAX_DICTATION_LIMIT_MIN)
                    * 60,
            ),
            double_tap: settings.double_tap_enabled,
        }
    }
}

impl Default for SessionPolicy {
    /// Used by external triggers (signals, CLI) that carry no settings
    /// snapshot: the spec defaults — 5 pending sessions, 5 minutes, double
    /// tap enabled.
    fn default() -> Self {
        Self {
            queue_capacity: 5,
            max_duration: Duration::from_secs(300),
            double_tap: true,
        }
    }
}

/// Intermediate pipeline stages reported back to the coordinator by the
/// transcription pipeline (`actions.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelinePhase {
    /// Text cleanup / post-processing started (raw transcript exists).
    Processing,
    /// Insertion into the target application started.
    Inserting,
}

/// How the pipeline ended, reported once per session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PipelineOutcome {
    /// Text was inserted (or accepted) — dwells in `Done`.
    Done,
    /// Transcription or insertion failed; audio is preserved upstream for
    /// "Tentar novamente" (FR-002-18, AC-002-09). Carries a short, log-safe
    /// error description for the Flow Bar.
    Failed(String),
    /// Nothing to transcribe: audio under 300 ms or blank transcript —
    /// "Nada ouvido" (FR-002-14).
    Empty,
    /// Cancelled (Esc / cancel command / session lock).
    Cancelled,
}

/// Payload of `session://state` (contracts.md §5): `{ session_id, state,
/// mode, error? }` plus optional `notice` and `pending` for the Flow Bar.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub(crate) struct SessionStateEvent {
    /// Id of the session the event belongs to. For `idle` events it is the
    /// id of the session that just ended; `None` only before the first one.
    pub session_id: Option<String>,
    /// "idle" | "arming" | "recording" | "transcribing" | "processing" |
    /// "inserting" | "done" | "error"
    pub state: &'static str,
    /// Session mode; only dictation exists in v1 (command/note are v1.1+).
    pub mode: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// One-shot notices: "duration_warning" (T-60s), "nothing_heard",
    /// "cancelled", "session_queue_full".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
    /// Pending sessions waiting for the pipeline to drain (FR-002-16).
    pub pending: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PttAction {
    Passthrough,
    DeferRelease,
    CancelRelease,
}

/// A key-up deferred by `RELEASE_GRACE` so a synthesized X11 auto-repeat
/// press can cancel it (#1539). When the grace elapses the hold is resolved
/// by [`CoordinatorState::finish_hold`] (recording) or
/// [`CoordinatorState::finish_pending_hold`] (press remembered while busy).
struct PendingRelease {
    binding_id: String,
    /// Activation mode of the press that armed this release — decides what a
    /// short hold means (push-to-talk tap vs hold-or-toggle tap).
    mode: ShortcutActivation,
    /// Holds at least this long are "holds"; shorter ones are "taps".
    /// Push-to-talk resolves against `ARMING_WINDOW` (the spec's 250 ms tap
    /// boundary), hold-or-toggle against `hold_threshold`.
    tap_boundary: Duration,
    deadline: Instant,
    /// When the key actually went up. The hold duration is measured to this
    /// instant, not to the grace expiry.
    released_at: Instant,
}

/// A press that arrived while the pipeline was still busy. Toggle-style
/// triggers (SIGUSR2, CLI flags, some pedal setups) flip state on every edge,
/// so dropping a busy press desyncs the parity: the next edge starts a
/// recording nobody will ever stop. FR-002-16 turns the single remembered
/// press into a FIFO queue of up to `session_queue_size` pending sessions.
#[derive(Debug)]
struct PendingPress {
    binding_id: String,
    hotkey_string: String,
    /// The real key-down time, so a hold that straddles the drain is still
    /// measured from when the user pressed, not from when recording began.
    pressed_at: Instant,
    /// The recording will start locked on when the pipeline drains: set from
    /// the start for toggle, and for hold-or-toggle once the key came back up
    /// within the tap boundary (a tap). An unlocked pending press is a key we
    /// believe is still held.
    locked: bool,
    /// Policy snapshot at press time (duration limit / double-tap behaviour).
    policy: SessionPolicy,
}

impl PendingPress {
    fn remembered(&self) -> Remembered {
        if self.locked {
            Remembered::Locked
        } else {
            Remembered::Held
        }
    }
}

/// What kind of press is already waiting for the pipeline to drain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Remembered {
    /// The key is still down as far as we know.
    Held,
    /// A toggle press or a classified tap: it will start a locked session.
    Locked,
}

/// A live session. `Session` is carried by every non-`Idle` stage so the id
/// and capture start survive all the way to `Done`/`Error`.
#[derive(Debug)]
pub(crate) struct Session {
    id: String,
    binding_id: String,
    hotkey_string: String,
    /// Real key-down time: the tap-vs-hold measurement base.
    pressed_at: Instant,
    /// When capture began — the base for the duration limit and a good proxy
    /// for the audio length (recording starts in `Arming`, FR-002-10).
    capture_at: Instant,
    /// Session outlives the key: the next press stops it, releases are
    /// ignored. Always set for toggle; set for hold-or-toggle once a release
    /// was classified as a tap; set for push-to-talk on a double tap.
    locked: bool,
    max_duration: Duration,
    double_tap: bool,
    /// Set when a short push-to-talk press was released inside the arming
    /// window: the session keeps capturing until `released + DOUBLE_TAP_WINDOW`
    /// waiting for the second tap that locks hands-free (FR-002-07).
    awaiting_second_tap_since: Option<Instant>,
    /// Whether the T-60s duration warning was already emitted.
    duration_warned: bool,
}

/// What to do with an input that arrives while the pipeline is busy.
/// `remembered` is the press *for the same binding* already waiting in the
/// queue, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BusyAction {
    /// Ignore the input entirely.
    Ignore,
    /// Remember the press; start recording when the pipeline finishes.
    Remember,
    /// This press cancels a previously remembered press: two presses during
    /// one busy window net to no-op, exactly as a press stops a locked
    /// session once recording.
    Forget,
}

fn classify_busy_input(
    is_pressed: bool,
    mode: ShortcutActivation,
    remembered: Option<Remembered>,
) -> BusyAction {
    use ShortcutActivation::*;
    match (mode, is_pressed, remembered) {
        // Toggle: presses alternate remember/forget to preserve parity.
        (Toggle, true, Some(_)) => BusyAction::Forget,
        (Toggle, true, None) => BusyAction::Remember,
        // Toggle mode ignores releases.
        (Toggle, false, _) => BusyAction::Ignore,
        // Hold modes: a press while busy means the user is holding the key —
        // start as soon as the pipeline drains. A press on a queued tap stops
        // it (parity); a press while the key is already down is a repeat.
        (PushToTalk | HoldOrToggle, true, None) => BusyAction::Remember,
        (PushToTalk | HoldOrToggle, true, Some(Remembered::Locked)) => BusyAction::Forget,
        (PushToTalk | HoldOrToggle, true, Some(Remembered::Held)) => BusyAction::Ignore,
        // Releases of a held pending press are deferred by the grace window
        // before reaching here and resolved by `finish_pending_hold`; any
        // other release (no press remembered, or already locked) is noise.
        (PushToTalk | HoldOrToggle, false, _) => BusyAction::Ignore,
    }
}

/// Session lifecycle — the full FR-002-09 machine.
#[derive(Debug)]
pub(crate) enum Stage {
    Idle,
    /// Capture on, session uncommitted (arming window / awaiting second tap).
    Arming(Session),
    /// Committed capture.
    Recording(Session),
    /// Stop requested; audio goes to the provider.
    Transcribing(Session),
    /// Text cleanup / post-processing.
    Processing(Session),
    /// Insertion into the target application.
    Inserting(Session),
    /// Completed; dwells `DONE_DWELL` then returns to `Idle`.
    Done(Session, Instant),
    /// Failed; dwells `ERROR_DWELL` then returns to `Idle`. The error text
    /// lives on the emitted `session://state` event and the persisted
    /// `failed` dictation row — the stage only needs the dwell deadline.
    Error(Session, Instant),
}

impl Stage {
    /// contracts.md state name for `session://state` payloads.
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Stage::Idle => "idle",
            Stage::Arming(_) => "arming",
            Stage::Recording(_) => "recording",
            Stage::Transcribing(_) => "transcribing",
            Stage::Processing(_) => "processing",
            Stage::Inserting(_) => "inserting",
            Stage::Done(..) => "done",
            Stage::Error(..) => "error",
        }
    }

    fn session(&self) -> Option<&Session> {
        match self {
            Stage::Idle => None,
            Stage::Arming(s)
            | Stage::Recording(s)
            | Stage::Transcribing(s)
            | Stage::Processing(s)
            | Stage::Inserting(s)
            | Stage::Done(s, _)
            | Stage::Error(s, ..) => Some(s),
        }
    }

    /// A session is capturing audio.
    fn is_capturing(&self) -> bool {
        matches!(self, Stage::Arming(_) | Stage::Recording(_))
    }

    /// The pipeline owns the session: presses queue up instead of starting.
    fn is_busy(&self) -> bool {
        matches!(
            self,
            Stage::Transcribing(_) | Stage::Processing(_) | Stage::Inserting(_)
        )
    }

    /// Terminal dwell: the pipeline is free, the outcome is still on display.
    fn is_dwelling(&self) -> bool {
        matches!(self, Stage::Done(..) | Stage::Error(..))
    }
}

/// A keyboard/signal edge for a transcribe binding.
pub(crate) struct InputEvent {
    pub binding_id: String,
    pub hotkey_string: String,
    pub is_pressed: bool,
    pub mode: ShortcutActivation,
    /// Hold-or-toggle: minimum press duration that counts as a hold.
    pub hold_threshold: Duration,
    /// External triggers (SIGUSR2, CLI flags) rather than physical keys.
    /// They fire on every edge by design and must never be debounced —
    /// dropping one desyncs toggle parity and wedges recording on.
    pub external: bool,
    /// Session limits snapshot (queue capacity, duration limit, double-tap).
    pub policy: SessionPolicy,
}

/// The boundary below which a release is a "tap" rather than a "hold".
/// Push-to-talk resolves taps against the spec's 250 ms arming window
/// (a shorter press is never a dictation, AC-002-02); hold-or-toggle uses
/// the configured hold threshold.
fn tap_boundary(mode: ShortcutActivation, hold_threshold: Duration) -> Duration {
    match mode {
        ShortcutActivation::PushToTalk => ARMING_WINDOW,
        ShortcutActivation::HoldOrToggle | ShortcutActivation::Toggle => hold_threshold,
    }
}

/// A side effect decided by [`CoordinatorState`]; the coordinator thread is
/// the only executor. Keeping decisions pure lets tests drive the exact
/// production transitions without a Tauri `AppHandle` or real timers.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    /// Begin audio capture (state enters `Arming`).
    Start {
        binding_id: String,
        hotkey_string: String,
    },
    /// End capture and run the pipeline (state enters `Transcribing`).
    Stop {
        binding_id: String,
        hotkey_string: String,
    },
    /// Discard the capture without transcribing (arming cancel, short tap
    /// without a second tap). Silent per AC-002-02/AC-002-04.
    Discard { binding_id: String },
}

/// Decide whether a key-up should be deferred (so auto-repeat can cancel it)
/// or a key-down cancels a deferred release. `hold_to_talk` is whether a
/// release currently ends the session: true for push-to-talk and for an
/// unlocked hold-or-toggle session, false for toggle and a locked session.
/// `held_binding` is the binding whose key we believe is down — the one
/// recording, or the one remembered while the pipeline is busy.
fn classify_ptt_event(
    pending_release_binding: Option<&str>,
    is_pressed: bool,
    hold_to_talk: bool,
    binding_id: &str,
    held_binding: Option<&str>,
) -> PttAction {
    if !hold_to_talk {
        return PttAction::Passthrough;
    }

    if is_pressed {
        if pending_release_binding == Some(binding_id) {
            PttAction::CancelRelease
        } else {
            PttAction::Passthrough
        }
    } else if held_binding == Some(binding_id) && pending_release_binding.is_none() {
        PttAction::DeferRelease
    } else {
        PttAction::Passthrough
    }
}

/// `session://result` payload (contracts.md §5): emitted by the pipeline
/// when a session delivers its text (or definitively fails).
#[derive(Clone, Debug, serde::Serialize)]
pub struct SessionResultEvent {
    pub session_id: String,
    pub final_text: String,
    /// True only when the text was actually injected (`inserted`); a
    /// `copied` outcome (clipboard_only) leaves this false.
    pub inserted: bool,
    /// FR-005-11: `inserted` | `copied` | `failed`; `None` when no insertion
    /// was attempted (e.g. an STT failure upstream).
    pub insertion_status: Option<String>,
    /// Effective insertion method after `auto` resolution and UIPI fallback
    /// (`paste`, `paste_shift_insert`, `paste_ctrl_shift_v`, `type`,
    /// `clipboard_only`, `external_script`).
    pub insertion_method: Option<String>,
    /// Why the plan degraded to `clipboard_only`
    /// (`elevated_target` | `no_foreground_target` | `window_changed` |
    /// `requested`).
    pub insertion_fallback: Option<String>,
    /// Insert dispatch wall time in milliseconds.
    pub insert_ms: Option<u64>,
}

/// Snapshot of the active session shared with the pipeline (`actions.rs`):
/// the id carried by `session://result` and the capture start used for the
/// minimum-audio-duration check (FR-002-14) and `duration_ms` in dictations.
#[derive(Clone, Debug)]
pub struct SessionSnapshot {
    pub id: String,
    pub binding_id: String,
    pub capture_started_at: Instant,
}

/// The pure lifecycle state machine. See the module docs for the full
/// transition table.
pub(crate) struct CoordinatorState {
    stage: Stage,
    last_press: Option<Instant>,
    pending_release: Option<PendingRelease>,
    /// FIFO of sessions remembered while the pipeline is busy (FR-002-16),
    /// bounded by `SessionPolicy::queue_capacity`.
    pending: VecDeque<PendingPress>,
    /// Capacity seen on the most recent busy press.
    queue_capacity: usize,
    next_session_id: u64,
    /// Journal of `session://state` payloads; the coordinator thread drains
    /// it after every command and emits each entry in order.
    events: Vec<SessionStateEvent>,
    /// Session id to attach to `idle` events.
    last_session_id: Option<String>,
}

impl CoordinatorState {
    pub(crate) fn new() -> Self {
        Self {
            stage: Stage::Idle,
            last_press: None,
            pending_release: None,
            pending: VecDeque::new(),
            queue_capacity: SessionPolicy::default().queue_capacity,
            next_session_id: 1,
            events: Vec::new(),
            last_session_id: None,
        }
    }

    /// Drain the emitted `session://state` payloads (in order).
    pub(crate) fn take_events(&mut self) -> Vec<SessionStateEvent> {
        std::mem::take(&mut self.events)
    }

    /// Snapshot of the active session for pipeline consumers.
    pub(crate) fn current_session_snapshot(&self) -> Option<SessionSnapshot> {
        self.stage.session().map(|s| SessionSnapshot {
            id: s.id.clone(),
            binding_id: s.binding_id.clone(),
            capture_started_at: s.capture_at,
        })
    }

    /// Earliest instant at which a timer must fire — drives `recv_timeout`.
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        let mut deadline = self.pending_release.as_ref().map(|p| p.deadline);
        let consider = |d: &mut Option<Instant>, candidate: Instant| {
            *d = Some(d.map_or(candidate, |d0| d0.min(candidate)));
        };

        if let Some(session) = self.stage.session().filter(|_| self.stage.is_capturing()) {
            if let Some(released) = session.awaiting_second_tap_since {
                consider(&mut deadline, released + DOUBLE_TAP_WINDOW);
            } else if matches!(self.stage, Stage::Arming(_)) {
                consider(&mut deadline, session.pressed_at + ARMING_WINDOW);
            }
            if !session.duration_warned {
                consider(
                    &mut deadline,
                    session.capture_at + session.max_duration.saturating_sub(DURATION_WARN_LEAD),
                );
            }
            consider(&mut deadline, session.capture_at + session.max_duration);
        }

        if let Stage::Done(_, until) | Stage::Error(.., until) = &self.stage {
            consider(&mut deadline, *until);
        }

        deadline
    }

    /// The capturing session, if any.
    fn capturing(&self) -> Option<&Session> {
        self.stage.session().filter(|_| self.stage.is_capturing())
    }

    /// Whether the current session (recording, or remembered for the drain)
    /// outlives the key, so releases are ignored and the next press ends it.
    fn is_locked(&self) -> bool {
        self.capturing().is_some_and(|s| s.locked) || self.pending.iter().any(|p| p.locked)
    }

    /// Whether a release of `binding_id` would currently end a session.
    /// Per-binding during the busy window: a queued held press still resolves
    /// its release even while another queued press is locked.
    fn hold_to_talk_for(&self, input: &InputEvent) -> bool {
        if input.mode == ShortcutActivation::Toggle {
            return false;
        }
        if self.stage.is_busy() {
            return !self
                .pending
                .iter()
                .rev()
                .find(|p| p.binding_id == input.binding_id)
                .is_some_and(|p| p.locked);
        }
        !self.is_locked()
    }

    /// The binding whose key is currently held for deferral purposes: the
    /// capturing binding, or the pending queue entry for this binding.
    fn held_binding<'a>(&'a self, binding_id: &'a str) -> Option<&'a str> {
        if let Some(session) = self.capturing() {
            return Some(session.binding_id.as_str());
        }
        if self.stage.is_busy() {
            return self
                .pending
                .iter()
                .rev()
                .find(|p| p.binding_id == binding_id)
                .map(|p| p.binding_id.as_str());
        }
        None
    }

    /// Push a `session://state` payload reflecting the current stage.
    fn push_event(&mut self, error: Option<String>, notice: Option<&'static str>) {
        self.events.push(SessionStateEvent {
            session_id: self
                .stage
                .session()
                .map(|s| s.id.clone())
                .or_else(|| self.last_session_id.clone()),
            state: self.stage.name(),
            mode: "dictation",
            error,
            notice: notice.map(str::to_string),
            pending: self.pending.len(),
        });
    }

    /// Move to a new stage and emit the corresponding `session://state`.
    fn transition_to(&mut self, stage: Stage, error: Option<String>, notice: Option<&'static str>) {
        if let Some(s) = stage.session() {
            self.last_session_id = Some(s.id.clone());
        }
        self.stage = stage;
        self.push_event(error, notice);
    }

    /// Optimistic transition to `Arming` on a fresh press; rolled back via
    /// [`CoordinatorState::on_start_result`] if capture fails to start.
    fn begin_session(&mut self, input: InputEvent, now: Instant) -> Effect {
        let session = Session {
            id: self.alloc_session_id(),
            binding_id: input.binding_id.clone(),
            hotkey_string: input.hotkey_string.clone(),
            pressed_at: now,
            capture_at: now,
            // Toggle never ends on a release: locked from the start.
            locked: input.mode == ShortcutActivation::Toggle,
            max_duration: input.policy.max_duration,
            double_tap: input.policy.double_tap,
            awaiting_second_tap_since: None,
            duration_warned: false,
        };
        let effect = Effect::Start {
            binding_id: session.binding_id.clone(),
            hotkey_string: session.hotkey_string.clone(),
        };
        self.transition_to(Stage::Arming(session), None, None);
        effect
    }

    /// Start a session for a press remembered while the pipeline was busy.
    /// `pressed_at` keeps the real key-down so holds that straddled the drain
    /// are measured correctly; the duration limit counts from capture start.
    fn begin_pending_session(&mut self, pending: PendingPress, now: Instant) -> Effect {
        let session = Session {
            id: self.alloc_session_id(),
            binding_id: pending.binding_id.clone(),
            hotkey_string: pending.hotkey_string.clone(),
            pressed_at: pending.pressed_at,
            capture_at: now,
            locked: pending.locked,
            max_duration: pending.policy.max_duration,
            double_tap: pending.policy.double_tap,
            awaiting_second_tap_since: None,
            duration_warned: false,
        };
        let effect = Effect::Start {
            binding_id: session.binding_id.clone(),
            hotkey_string: session.hotkey_string.clone(),
        };
        self.transition_to(Stage::Arming(session), None, None);
        effect
    }

    fn alloc_session_id(&mut self) -> String {
        let id = format!("session-{:04}", self.next_session_id);
        self.next_session_id += 1;
        id
    }

    /// `Arming → Recording`: the session is confirmed (held ≥ 250 ms,
    /// double tap, or tap-locked hands-free).
    fn commit_recording(&mut self) {
        let stage = std::mem::replace(&mut self.stage, Stage::Idle);
        match stage {
            Stage::Arming(session) => self.transition_to(Stage::Recording(session), None, None),
            other => self.stage = other,
        }
    }

    /// `Arming|Recording → Transcribing`: graceful end of capture — release,
    /// stop press, or the duration limit (FR-002-13 processes normally).
    fn finish_capture(&mut self) -> Option<Effect> {
        let stage = std::mem::replace(&mut self.stage, Stage::Idle);
        let session = match stage {
            Stage::Arming(s) | Stage::Recording(s) => s,
            other => {
                self.stage = other;
                return None;
            }
        };
        let effect = Effect::Stop {
            binding_id: session.binding_id.clone(),
            hotkey_string: session.hotkey_string.clone(),
        };
        self.transition_to(Stage::Transcribing(session), None, None);
        Some(effect)
    }

    /// Drop a capturing session without transcribing (arming cancel or a tap
    /// with no second tap). Audio is discarded by the effect executor.
    fn discard_session(&mut self) -> Option<Effect> {
        let stage = std::mem::replace(&mut self.stage, Stage::Idle);
        let session = match stage {
            Stage::Arming(s) | Stage::Recording(s) => s,
            other => {
                self.stage = other;
                return None;
            }
        };
        let binding_id = session.binding_id.clone();
        self.last_session_id = Some(session.id.clone());
        self.push_event(None, None); // stage is Idle now
        Some(Effect::Discard { binding_id })
    }

    pub(crate) fn on_input(&mut self, input: InputEvent, now: Instant) -> Option<Effect> {
        let pending_release_binding = self
            .pending_release
            .as_ref()
            .map(|pending| pending.binding_id.as_str());
        let held_binding = self.held_binding(&input.binding_id);
        // While a tap is waiting out the double-tap window the key is already
        // up — releases must not be deferred again.
        let hold_to_talk = self.hold_to_talk_for(&input)
            && !self
                .capturing()
                .is_some_and(|s| s.awaiting_second_tap_since.is_some());

        match classify_ptt_event(
            pending_release_binding,
            input.is_pressed,
            hold_to_talk,
            &input.binding_id,
            held_binding,
        ) {
            PttAction::CancelRelease => {
                self.pending_release = None;
                return None;
            }
            PttAction::DeferRelease => {
                self.pending_release = Some(PendingRelease {
                    tap_boundary: tap_boundary(input.mode, input.hold_threshold),
                    mode: input.mode,
                    binding_id: input.binding_id,
                    deadline: now + RELEASE_GRACE,
                    released_at: now,
                });
                return None;
            }
            PttAction::Passthrough => {}
        }

        // Debounce rapid-fire press events (key repeat / double-tap).
        // Releases in the hold modes are deferred above to absorb X11 auto-repeat.
        // External triggers are exempt: each one is a deliberate edge from the
        // user's own integration, and dropping it desyncs toggle parity.
        if input.is_pressed && !input.external {
            if self
                .last_press
                .is_some_and(|t| now.duration_since(t) < DEBOUNCE)
            {
                debug!("Debounced press for '{}'", input.binding_id);
                return None;
            }
            self.last_press = Some(now);
        }

        // A busy pipeline can't accept lifecycle changes now: remember the
        // input in the FIFO queue (or fold it into a remembered press)
        // instead of dropping it silently (FR-002-16).
        if self.stage.is_busy() {
            self.on_busy_input(input, now);
            return None;
        }

        if input.is_pressed {
            match &mut self.stage {
                // Done/Error still dwell for the UI, but the pipeline is
                // already free — a press starts a fresh session.
                Stage::Idle | Stage::Done(..) | Stage::Error(..) => {
                    return Some(self.begin_session(input, now));
                }
                Stage::Arming(session) | Stage::Recording(session)
                    if session.binding_id == input.binding_id =>
                {
                    if session.awaiting_second_tap_since.is_some() {
                        // Second tap inside the window: hands-free on
                        // (FR-002-07). The session is now locked in Recording.
                        session.locked = true;
                        session.awaiting_second_tap_since = None;
                        self.commit_recording();
                        debug!(
                            "Double tap for '{}': session locked hands-free",
                            input.binding_id
                        );
                        return None;
                    }
                    // A locked session ends on the next press. In toggle mode
                    // every press ends it, even if the recording began under a
                    // hold mode (the setting changed mid-recording) — otherwise
                    // nothing but Escape could stop it.
                    if session.locked || input.mode == ShortcutActivation::Toggle {
                        return self.finish_capture();
                    }
                    // The key is still held (its release will end this
                    // recording), so a repeated press means nothing.
                    debug!("Ignoring press for '{}': key is held", input.binding_id);
                }
                _ => debug!(
                    "Ignoring press for '{}': another binding is active",
                    input.binding_id
                ),
            }
        } else {
            // A release that was not deferred (one is already pending for this
            // binding, or the machine was mid-flip): resolve it immediately
            // rather than dropping it.
            let is_capturing_binding = self
                .capturing()
                .is_some_and(|s| s.binding_id == input.binding_id);
            if hold_to_talk && is_capturing_binding {
                return self.finish_hold(PendingRelease {
                    binding_id: input.binding_id,
                    mode: input.mode,
                    tap_boundary: tap_boundary(input.mode, input.hold_threshold),
                    deadline: now,
                    released_at: now,
                });
            }
        }
        None
    }

    /// The `RELEASE_GRACE` window elapsed with no cancelling press arriving:
    /// resolve the deferred release against whatever that binding's key was
    /// holding — the live session, or a press remembered while busy.
    fn on_grace_expired(&mut self) -> Option<Effect> {
        let pending = self.pending_release.take()?;
        match &self.stage {
            Stage::Arming(s) | Stage::Recording(s) if s.binding_id == pending.binding_id => {
                self.finish_hold(pending)
            }
            stage if stage.is_busy() => {
                self.finish_pending_hold(&pending);
                None
            }
            _ => None,
        }
    }

    /// A press remembered while the pipeline was busy has been released for
    /// real, still before the drain. A completed hold has nothing left to
    /// start; a tap marks its queue entry locked so the drain starts a
    /// hands-free session — the same hold-vs-tap rule as
    /// [`CoordinatorState::finish_hold`].
    fn finish_pending_hold(&mut self, release: &PendingRelease) {
        let Some(pos) = self
            .pending
            .iter()
            .rposition(|p| p.binding_id == release.binding_id)
        else {
            return;
        };
        let held = release
            .released_at
            .saturating_duration_since(self.pending[pos].pressed_at);
        if held >= release.tap_boundary {
            debug!(
                "Forgetting queued press for '{}': released after a {held:?} hold while busy",
                release.binding_id
            );
            self.pending.remove(pos);
            return;
        }
        match release.mode {
            // A completed push-to-talk tap is never a session on its own —
            // the same arming rule as a live capture (AC-002-02).
            ShortcutActivation::PushToTalk => {
                debug!(
                    "Forgetting queued press for '{}': {held:?} tap while busy",
                    release.binding_id
                );
                self.pending.remove(pos);
            }
            // A hold-or-toggle tap marks its queue entry locked so the drain
            // starts a hands-free session.
            ShortcutActivation::HoldOrToggle | ShortcutActivation::Toggle => {
                debug!(
                    "Tap ({held:?}) for '{}' while busy: will start locked on when the pipeline drains",
                    release.binding_id
                );
                if let Some(p) = self.pending.get_mut(pos) {
                    p.locked = true;
                }
            }
        }
    }

    /// The key that started the current session has been released for real.
    /// A hold at least the tap boundary long stops capture; anything shorter
    /// is a tap: for hold-or-toggle it locks hands-free, for push-to-talk it
    /// waits out the double-tap window (or is discarded when double tap is
    /// off) — per FR-002-06/07 the arming release alone is never a dictation.
    fn finish_hold(&mut self, release: PendingRelease) -> Option<Effect> {
        let (held, double_tap) = {
            let session = self.capturing()?;
            (
                release
                    .released_at
                    .saturating_duration_since(session.pressed_at),
                session.double_tap,
            )
        };
        if held >= release.tap_boundary {
            return self.finish_capture();
        }
        match release.mode {
            ShortcutActivation::PushToTalk => {
                if !double_tap {
                    debug!(
                        "Tap ({held:?}) for '{}' with double tap off: discarding session",
                        release.binding_id
                    );
                    return self.discard_session();
                }
                // Wait out the double-tap window for a second tap.
                if let Some(s) = self.capturing_mut() {
                    s.awaiting_second_tap_since = Some(release.released_at);
                }
                // Stepping back to Arming makes the uncommitted state
                // visible again if the commit deadline already fired.
                if matches!(self.stage, Stage::Recording(_)) {
                    let stage = std::mem::replace(&mut self.stage, Stage::Idle);
                    if let Stage::Recording(s) = stage {
                        self.transition_to(Stage::Arming(s), None, None);
                    } else {
                        self.stage = stage;
                    }
                }
                debug!(
                    "Tap ({held:?}) for '{}': awaiting a second tap for {:?}",
                    release.binding_id, DOUBLE_TAP_WINDOW
                );
                None
            }
            // A tap in hold-or-toggle locks the session on until the next
            // press — hands-free per the configured activation mode.
            ShortcutActivation::HoldOrToggle | ShortcutActivation::Toggle => {
                if let Some(s) = self.capturing_mut() {
                    debug!(
                        "Tap ({held:?}) for '{}': recording locked on until the next press",
                        s.binding_id
                    );
                    s.locked = true;
                }
                self.commit_recording();
                None
            }
        }
    }

    fn capturing_mut(&mut self) -> Option<&mut Session> {
        match &mut self.stage {
            Stage::Arming(s) | Stage::Recording(s) => Some(s),
            _ => None,
        }
    }

    /// Input while the pipeline is busy (`Transcribing`/`Processing`/
    /// `Inserting`): maintain the FIFO of pending sessions (FR-002-16).
    fn on_busy_input(&mut self, input: InputEvent, now: Instant) {
        self.queue_capacity = input.policy.queue_capacity.max(1);
        let remembered = self
            .pending
            .iter()
            .rev()
            .find(|p| p.binding_id == input.binding_id)
            .map(|p| p.remembered());
        match classify_busy_input(input.is_pressed, input.mode, remembered) {
            BusyAction::Remember => {
                if self.pending.len() >= self.queue_capacity {
                    debug!(
                        "Session queue full ({} pending): rejecting press for '{}'",
                        self.queue_capacity, input.binding_id
                    );
                    self.push_event(None, Some("session_queue_full"));
                    return;
                }
                self.pending.push_back(PendingPress {
                    // Toggle never ends on a release: locked from the start.
                    locked: input.mode == ShortcutActivation::Toggle,
                    binding_id: input.binding_id,
                    hotkey_string: input.hotkey_string,
                    pressed_at: now,
                    policy: input.policy,
                });
                debug!("Queued pending session ({} pending)", self.pending.len());
            }
            BusyAction::Forget => {
                if let Some(pos) = self
                    .pending
                    .iter()
                    .rposition(|p| p.binding_id == input.binding_id)
                {
                    debug!("Forgetting queued press for '{}'", input.binding_id);
                    self.pending.remove(pos);
                }
            }
            BusyAction::Ignore => {
                debug!("Ignoring input for '{}': pipeline busy", input.binding_id);
            }
        }
    }

    pub(crate) fn on_cancel(&mut self, recording_was_active: bool) {
        self.pending_release = None;
        // An explicit cancel abandons remembered starts too — the user asked
        // for silence, not a deferred recording.
        self.pending.clear();
        // Don't reset while the pipeline runs — wait for it to finish.
        if self.stage.is_busy() {
            return;
        }
        if self.stage.is_capturing() || recording_was_active || self.stage.is_dwelling() {
            if self.stage.is_capturing() || self.stage.is_dwelling() {
                if let Some(s) = self.stage.session() {
                    self.last_session_id = Some(s.id.clone());
                }
            }
            self.stage = Stage::Idle;
            self.push_event(None, Some("cancelled"));
        }
    }

    /// Intermediate pipeline stage reported by `actions.rs` (Processing =
    /// text cleanup, Inserting = paste). Ignored outside the busy window so
    /// stale reports can't corrupt a new session.
    pub(crate) fn on_pipeline_phase(&mut self, phase: PipelinePhase) {
        if !self.stage.is_busy() {
            return;
        }
        let stage = std::mem::replace(&mut self.stage, Stage::Idle);
        let session = match stage {
            Stage::Transcribing(s) | Stage::Processing(s) | Stage::Inserting(s) => s,
            other => {
                self.stage = other;
                return;
            }
        };
        match phase {
            PipelinePhase::Processing => self.transition_to(Stage::Processing(session), None, None),
            PipelinePhase::Inserting => self.transition_to(Stage::Inserting(session), None, None),
        }
    }

    /// The pipeline finished: record the outcome (`Done`/`Error` dwell, the
    /// rest return to `Idle` immediately) then drain the FIFO — the next
    /// remembered session starts recording right away (FR-002-16).
    pub(crate) fn on_pipeline_finished(
        &mut self,
        outcome: PipelineOutcome,
        now: Instant,
    ) -> Option<Effect> {
        let stage = std::mem::replace(&mut self.stage, Stage::Idle);
        let session = match stage {
            Stage::Transcribing(s) | Stage::Processing(s) | Stage::Inserting(s) => Some(s),
            // Stale or repeated finish: keep the current stage, still drain.
            other => {
                self.stage = other;
                None
            }
        };

        if let Some(session) = session {
            match outcome {
                PipelineOutcome::Done => {
                    self.transition_to(Stage::Done(session, now + DONE_DWELL), None, None)
                }
                PipelineOutcome::Failed(error) => {
                    self.transition_to(Stage::Error(session, now + ERROR_DWELL), Some(error), None)
                }
                PipelineOutcome::Empty => {
                    self.last_session_id = Some(session.id.clone());
                    self.stage = Stage::Idle;
                    self.push_event(None, Some("nothing_heard"));
                }
                PipelineOutcome::Cancelled => {
                    self.last_session_id = Some(session.id.clone());
                    self.stage = Stage::Idle;
                    self.push_event(None, Some("cancelled"));
                }
            }
        }

        self.drain_queue(now)
    }

    /// Pop the oldest remembered press and start its session (FIFO).
    fn drain_queue(&mut self, now: Instant) -> Option<Effect> {
        let pending = self.pending.pop_front()?;
        debug!(
            "Pipeline drained; starting queued session for '{}' ({} left)",
            pending.binding_id,
            self.pending.len()
        );
        Some(self.begin_pending_session(pending, now))
    }

    /// A timer fired: release grace, arming commit, second-tap window,
    /// duration warning/limit, or a Done/Error dwell. Returns at most one
    /// pipeline effect; the caller re-arms `next_deadline` for the rest.
    pub(crate) fn on_deadline(&mut self, now: Instant) -> Option<Effect> {
        if self
            .pending_release
            .as_ref()
            .is_some_and(|p| p.deadline <= now)
        {
            return self.on_grace_expired();
        }

        // Snapshot the due timers of a capturing session.
        let (tap_window_expired, commit_due, warn_due, limit_due) = match &self.stage {
            Stage::Arming(s) => (
                s.awaiting_second_tap_since
                    .is_some_and(|t| t + DOUBLE_TAP_WINDOW <= now),
                s.awaiting_second_tap_since.is_none() && s.pressed_at + ARMING_WINDOW <= now,
                !s.duration_warned
                    && s.capture_at + s.max_duration.saturating_sub(DURATION_WARN_LEAD) <= now,
                s.capture_at + s.max_duration <= now,
            ),
            Stage::Recording(s) => (
                s.awaiting_second_tap_since
                    .is_some_and(|t| t + DOUBLE_TAP_WINDOW <= now),
                false,
                !s.duration_warned
                    && s.capture_at + s.max_duration.saturating_sub(DURATION_WARN_LEAD) <= now,
                s.capture_at + s.max_duration <= now,
            ),
            _ => (false, false, false, false),
        };

        // No second tap arrived: the tap was not a dictation — discard.
        if tap_window_expired {
            debug!("Second-tap window expired: discarding armed session");
            return self.discard_session();
        }
        if commit_due {
            self.commit_recording();
        }
        // FR-002-13: at the limit the session ends and processes normally.
        if limit_due {
            debug!("Duration limit reached: ending capture gracefully");
            if let Some(s) = self.capturing_mut() {
                s.duration_warned = true;
            }
            return self.finish_capture();
        }
        // FR-002-13: T-60s countdown warning for the Flow Bar.
        if warn_due {
            if let Some(s) = self.capturing_mut() {
                s.duration_warned = true;
            }
            self.push_event(None, Some("duration_warning"));
        }

        // Done/Error dwell elapsed → back to Idle.
        let dwell_expired = match &self.stage {
            Stage::Done(_, until) | Stage::Error(.., until) => *until <= now,
            _ => false,
        };
        if dwell_expired {
            self.stage = Stage::Idle;
            self.push_event(None, None);
        }
        None
    }

    /// Reconcile the optimistic `Arming` after the executor reports whether
    /// recording actually began (microphone access can be denied). On failure
    /// the session becomes `Error` briefly and the queue is abandoned —
    /// queued sessions would hit the same capture error.
    pub(crate) fn on_start_result(&mut self, binding_id: &str, started: bool, now: Instant) {
        if started {
            return;
        }
        let owns = self.capturing().is_some_and(|s| s.binding_id == binding_id);
        if owns {
            self.pending.clear();
            let stage = std::mem::replace(&mut self.stage, Stage::Idle);
            if let Stage::Arming(s) | Stage::Recording(s) = stage {
                self.transition_to(
                    Stage::Error(s, now + ERROR_DWELL),
                    Some("start_failed".to_string()),
                    None,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests;
