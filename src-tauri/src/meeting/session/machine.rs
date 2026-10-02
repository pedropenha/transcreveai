//! Pure session machine for an active meeting (T-064; FR-009-01/02,
//! FR-009-06..09, AC-009-08).
//!
//! Owns every lifecycle and timing decision of the one meeting that may be
//! active at a time: `recording ⇄ paused → processing | error`, the duration
//! limit with its T-5 min "Estender 30 min" warning (FR-009-08), the 10-min
//! silence check-in with its 2-min answer window (FR-009-09), pause/device
//! `gap_marker`s, and write-failure escalation (disk-full edge case). It is
//! deliberately free of I/O — every method takes the current `Instant` and
//! journals [`Effect`]s + [`MeetingStateEvent`]s; the worker in
//! `super::worker` is the thin effect shell (the
//! `transcription_coordinator` split).

use std::time::{Duration, Instant};

use serde::Serialize;

use crate::meeting::blocks::Track;

/// `meeting://state` — contracts.md §5. On every transition and once per
/// second while recording (the Flow Bar timer reads `elapsed_ms`).
pub const MEETING_STATE_EVENT: &str = "meeting://state";
/// Seam for T-067: emitted once when a meeting enters `processing`.
pub const MEETING_PROCESS_REQUESTED_EVENT: &str = "meeting://process-requested";
/// `toast://show` — rendered by the toast window (T-062).
pub const TOAST_SHOW_EVENT: &str = "toast://show";
/// T-061 emits this when the detector's toast asks to start a meeting.
pub const DETECTOR_START_REQUESTED_EVENT: &str = "detector://start-requested";
/// How often `meeting://state` is re-emitted while recording.
pub const STATE_TICK: Duration = Duration::from_secs(1);
/// FR-009-08: the limit warning fires this long before the deadline.
pub const LIMIT_WARNING_LEAD: Duration = Duration::from_secs(5 * 60);
/// FR-009-08: "Estender 30 min".
pub const EXTEND_BY: Duration = Duration::from_secs(30 * 60);
/// FR-009-09: silence on every track for this long raises the check-in.
pub const SILENCE_CHECKIN_AFTER: Duration = Duration::from_secs(10 * 60);
/// FR-009-09: unanswered check-ins stop the meeting after this.
pub const CHECKIN_RESPONSE_WINDOW: Duration = Duration::from_secs(2 * 60);
/// Consecutive block-write failures before the session stops with `error`
/// (disk-full edge case — sealed blocks stay valid).
pub const WRITE_FAILURE_LIMIT: u32 = 3;
/// FR-008-14 (T-069): when the linked detection reports the meeting over,
/// the "Continuar gravando" affordance gets this long before the session
/// stops on its own.
pub const AUTO_STOP_GRACE: Duration = Duration::from_secs(15);
/// FR-009-08 options: 30 min / 1 h / 2 h / 3 h / 4 h.
pub const MEETING_LIMIT_OPTIONS_MIN: [u64; 5] = [30, 60, 120, 180, 240];
pub const DEFAULT_MEETING_LIMIT_MIN: u64 = 120;

/// Snap `meeting_max_minutes` onto the nearest spec option (FR-009-08).
/// Ties round up (45 → 60): never undercut the limit the user picked.
pub fn clamp_meeting_max_minutes(minutes: u64) -> u64 {
    MEETING_LIMIT_OPTIONS_MIN
        .iter()
        .copied()
        .min_by(|a, b| {
            a.abs_diff(minutes)
                .cmp(&b.abs_diff(minutes))
                .then_with(|| b.cmp(a))
        })
        .unwrap_or(DEFAULT_MEETING_LIMIT_MIN)
}

/// `meeting://state` payload (contracts.md §5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, specta::Type)]
pub struct MeetingStateEvent {
    pub meeting_id: String,
    /// 'recording' | 'paused' | 'processing' | 'ready' | 'error' | 'recovered'
    pub status: String,
    /// Milliseconds since the meeting started (pause time included).
    pub elapsed_ms: u64,
}

/// Toasts the machine asks the worker to emit (`toast://show`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    /// Mic held exclusively by another app — meeting keeps system (edge case).
    MicUnavailable,
    /// "Os outros participantes não estão sendo capturados".
    SystemUnavailable,
    /// FR-009-08 T-5 min warning — `action: "extend_30"`.
    LimitWarning,
    /// FR-009-09 "Ainda em reunião?" — `action: "checkin"`.
    SilenceCheckin,
    /// Repeated block-write failures — meeting stopped with `error`.
    WriteFailed,
    /// FR-008-14 (T-069) "A reunião terminou — finalizando em 15 s" —
    /// `action: "continue_recording"`.
    AutoStop,
    /// FR-009-02: the discreet post-start reminder — `action: "copy_consent"`
    /// puts the configured notice on the clipboard. Shares its `kind()`
    /// string with the consent *gate* refusal toast (`open_consent`), which
    /// MeetingConsentGate uses to tell them apart.
    ConsentReminder,
}

impl ToastKind {
    /// `toast://show.kind` discriminator.
    pub fn kind(self) -> &'static str {
        match self {
            ToastKind::MicUnavailable | ToastKind::SystemUnavailable | ToastKind::WriteFailed => {
                "meeting_warning"
            }
            ToastKind::LimitWarning => "meeting_limit",
            ToastKind::SilenceCheckin => "meeting_checkin",
            ToastKind::AutoStop => "meeting_auto_stop",
            ToastKind::ConsentReminder => "meeting_consent",
        }
    }

    /// `toast://show.action`, when the toast carries a button.
    pub fn action(self) -> Option<&'static str> {
        match self {
            ToastKind::LimitWarning => Some("extend_30"),
            ToastKind::SilenceCheckin => Some("checkin"),
            ToastKind::AutoStop => Some("continue_recording"),
            ToastKind::ConsentReminder => Some("copy_consent"),
            _ => None,
        }
    }
}

/// Why the machine stopped the meeting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    /// `meeting_stop` (user, tray, Flow Bar).
    User,
    /// Duration limit hit (FR-009-08).
    Limit,
    /// Check-in unanswered for 2 min (AC-009-08).
    CheckinTimeout,
    /// User pressed "Parar" on the check-in toast.
    CheckinDeclined,
    /// Consecutive block-write failures — likely a full disk.
    WriteFailures,
    /// FR-008-14 (T-069): the linked detection's meeting ended and the
    /// 15 s "Continuar gravando" window lapsed unanswered.
    MeetingEnded,
}

/// Side effects the worker performs for the machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// (Re)start the capture into the meeting dir — initial start or resume.
    /// On resume the worker continues the on-disk block indices.
    StartCapture,
    /// Seal the in-flight blocks and stop both tracks (pause or stop).
    StopCapture,
    /// Persist a `gap_marker` segment (`start_ms`/`end_ms` relative to the
    /// meeting start) — pause spans land on `mic`, device-swap spans
    /// (FR-009-04) on `system`.
    WriteGapMarker {
        track: Track,
        start_ms: i64,
        end_ms: i64,
    },
    /// Persist the meeting row's `status`/`error_code`.
    PersistStatus {
        status: &'static str,
        error_code: Option<&'static str>,
    },
    /// Emit `toast://show` for this kind (the worker localizes the message).
    Toast(ToastKind),
    /// Close a still-showing notice of this kind — journaled when a meeting
    /// stops with a pending auto-stop/check-in prompt or when resumed speech
    /// cancels the check-in, so the stale notice does not linger.
    DismissToast(ToastKind),
    /// Emit `meeting://process-requested` for T-067's post-processing.
    RequestProcessing,
    /// Recording indicator on/off (tray icon, "Stop Meeting" row, Flow Bar
    /// pill — FR-009-07; never user-suppressible).
    Indicator(bool),
}

/// Snapshot of session policy, taken from settings at start (mirroring
/// `transcription_coordinator::SessionPolicy`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeetingPolicy {
    pub max_duration: Duration,
    pub silence_checkin: bool,
    /// `meeting_auto_stop` (FR-008-14, default on) — snapshotted at start,
    /// like the rest of the policy.
    pub auto_stop: bool,
}

impl MeetingPolicy {
    pub fn from_settings(settings: &crate::settings::AppSettings) -> Self {
        Self {
            max_duration: Duration::from_secs(
                clamp_meeting_max_minutes(settings.meeting_max_minutes) * 60,
            ),
            silence_checkin: settings.meeting_silence_checkin_enabled,
            auto_stop: settings.meeting_auto_stop,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    Recording,
    Paused,
    /// Terminal — the worker drops the machine after `stop`.
    Done,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Status::Recording => "recording",
            Status::Paused => "paused",
            Status::Done => "done",
        }
    }
}

fn slot(track: Track) -> usize {
    match track {
        Track::Mic => 0,
        Track::System => 1,
    }
}

/// The one active meeting. Constructed when `meeting_start` wins the
/// single-meeting gate; dropped on `stop`.
pub struct SessionMachine {
    meeting_id: String,
    /// The `detection_id` that triggered this start (FR-008-14 linkage) —
    /// `None` for manual/Hub starts, which can never auto-stop.
    detection_id: Option<String>,
    /// Meeting start — `elapsed_ms` and segment offsets are relative to it.
    t0: Instant,
    status: Status,
    policy: MeetingPolicy,
    limit_at: Instant,
    limit_warned: bool,
    /// Last speech instant per track; `None` = track not live (mic
    /// unavailable, in-person mode). The check-in fires only when *every*
    /// live track has been quiet for `SILENCE_CHECKIN_AFTER`.
    last_speech: [Option<Instant>; 2],
    /// Check-in toast shown at — unanswered after `CHECKIN_RESPONSE_WINDOW`
    /// the meeting stops.
    checkin_since: Option<Instant>,
    /// Pause began at (its span becomes the `gap_marker`).
    pause_since: Option<Instant>,
    /// System-track detach pending its reattach gap marker.
    detached_since: Option<Instant>,
    /// Consecutive `WriteFailed` events; reset by every sealed block.
    write_failures: u32,
    /// FR-008-14: the detection reported the meeting over at this instant —
    /// stop when the grace lapses. Unlike the silence clock this runs on
    /// wall time: pausing does not extend the user's window to answer.
    auto_stop_since: Option<Instant>,
    /// Next `meeting://state` tick.
    tick_at: Instant,
    effects: Vec<Effect>,
    events: Vec<MeetingStateEvent>,
}

impl SessionMachine {
    /// `mic_live`/`system_live` tell the silence check-in which tracks can
    /// carry speech at all. `detection_id` links the meeting to the
    /// `detector://meeting` detection that started it (FR-008-14).
    pub fn new(
        meeting_id: String,
        now: Instant,
        policy: MeetingPolicy,
        mic_live: bool,
        system_live: bool,
        detection_id: Option<String>,
    ) -> Self {
        let mut machine = Self {
            meeting_id,
            detection_id,
            t0: now,
            status: Status::Recording,
            policy,
            limit_at: now + policy.max_duration,
            limit_warned: false,
            last_speech: [mic_live.then_some(now), system_live.then_some(now)],
            checkin_since: None,
            pause_since: None,
            detached_since: None,
            write_failures: 0,
            auto_stop_since: None,
            tick_at: now + STATE_TICK,
            effects: vec![Effect::Indicator(true)],
            events: Vec::new(),
        };
        machine.push_event(now, "recording");
        machine
    }

    /// FR-009-06 pause: seal the capture and record when the gap started.
    pub fn pause(&mut self, now: Instant) {
        if self.status != Status::Recording {
            return;
        }
        self.status = Status::Paused;
        self.pause_since = Some(now);
        self.effects.push(Effect::StopCapture);
        self.push_status_effect("paused", None);
        self.push_event(now, "paused");
    }

    /// FR-009-06 resume: write the pause `gap_marker`, restart the capture on
    /// the meeting's monotonic base, and shift every silence deadline forward
    /// by the pause span — the silence clock freezes while paused.
    pub fn resume(&mut self, now: Instant) {
        if self.status != Status::Paused {
            return;
        }
        let Some(pause_since) = self.pause_since.take() else {
            return;
        };
        let gap = now.saturating_duration_since(pause_since);
        for last in self.last_speech.iter_mut().flatten() {
            *last += gap;
        }
        if let Some(since) = self.checkin_since.as_mut() {
            *since += gap;
        }
        self.status = Status::Recording;
        self.tick_at = now + STATE_TICK;
        self.effects.push(Effect::WriteGapMarker {
            track: Track::Mic,
            start_ms: self.ms(pause_since),
            end_ms: self.ms(now),
        });
        self.effects.push(Effect::StartCapture);
        self.push_status_effect("recording", None);
        self.push_event(now, "recording");
    }

    /// Stop the meeting — user, limit, check-in timeout/decline or a burst of
    /// write failures. `WriteFailures` lands on `error` (audio is preserved;
    /// the other reasons all reach `processing` for T-067.
    pub fn stop(&mut self, now: Instant, reason: StopReason) {
        if self.status == Status::Done {
            return;
        }
        self.status = Status::Done;
        // Whatever ended it, pending prompts are now stale.
        if self.auto_stop_since.take().is_some() {
            self.effects.push(Effect::DismissToast(ToastKind::AutoStop));
        }
        if self.checkin_since.take().is_some() {
            self.effects
                .push(Effect::DismissToast(ToastKind::SilenceCheckin));
        }
        self.effects.push(Effect::StopCapture);
        match reason {
            StopReason::WriteFailures => {
                self.effects.push(Effect::Toast(ToastKind::WriteFailed));
                self.push_status_effect("error", Some("write_failed"));
                self.push_event(now, "error");
            }
            _ => {
                self.push_status_effect("processing", None);
                self.push_event(now, "processing");
                self.effects.push(Effect::RequestProcessing);
            }
        }
        self.effects.push(Effect::Indicator(false));
    }

    /// FR-009-08 "Estender 30 min" — the warning re-arms when the new
    /// deadline is again more than 5 min away.
    pub fn extend(&mut self, now: Instant) {
        if self.status == Status::Done {
            return;
        }
        self.limit_at += EXTEND_BY;
        self.limit_warned = now + LIMIT_WARNING_LEAD >= self.limit_at;
    }

    /// A sealed block landed; `has_speech` is the worker's `SpeechSignal`
    /// verdict for its content.
    pub fn block_sealed(&mut self, track: Track, has_speech: bool, now: Instant) {
        self.write_failures = 0;
        if has_speech {
            self.last_speech[slot(track)] = Some(now);
            // Speech resumed — a pending check-in is moot; its toast closes.
            if self.checkin_since.take().is_some() {
                self.effects
                    .push(Effect::DismissToast(ToastKind::SilenceCheckin));
            }
        }
    }

    /// `MeetingCaptureEvent::TrackUnavailable` — warn and mark the track as
    /// not speech-capable so it can never hold the check-in open.
    pub fn track_unavailable(&mut self, track: Track) {
        self.last_speech[slot(track)] = None;
        self.effects.push(Effect::Toast(match track {
            Track::Mic => ToastKind::MicUnavailable,
            Track::System => ToastKind::SystemUnavailable,
        }));
    }

    /// A block failed to write/fsync; a burst of them means "stop safely"
    /// (disk full, spec edge case).
    pub fn write_failed(&mut self, now: Instant) {
        self.write_failures += 1;
        if self.write_failures >= WRITE_FAILURE_LIMIT {
            self.stop(now, StopReason::WriteFailures);
        }
    }

    /// FR-009-04: the system track stopped producing frames (device swap or
    /// unplug). Remember the instant so the reattach becomes a `gap_marker`.
    pub fn system_detached(&mut self, now: Instant) {
        self.detached_since = Some(now);
    }

    /// System track reattached. `after_gap` is the loopback's measured
    /// outage; fall back to the detach instant when it is absent.
    pub fn system_attached(&mut self, now: Instant, after_gap: Option<Duration>) {
        if self.status == Status::Done {
            return;
        }
        let start = after_gap
            .map(|gap| self.ms(now) - gap.as_millis() as i64)
            .or_else(|| self.detached_since.take().map(|d| self.ms(d)));
        if let Some(start_ms) = start.filter(|s| *s >= 0) {
            let end_ms = self.ms(now);
            if end_ms > start_ms {
                self.effects.push(Effect::WriteGapMarker {
                    track: Track::System,
                    start_ms,
                    end_ms,
                });
            }
        }
        self.detached_since = None;
    }

    /// `meeting_checkin_respond{ continue }` — `false` stops and processes.
    pub fn checkin_respond(&mut self, keep_recording: bool, now: Instant) {
        if self.status == Status::Done || self.checkin_since.is_none() {
            return;
        }
        self.effects
            .push(Effect::DismissToast(ToastKind::SilenceCheckin));
        if keep_recording {
            self.checkin_since = None;
            // The answer itself counts as presence: restart the 10-min clock.
            for last in self.last_speech.iter_mut().flatten() {
                *last = now;
            }
        } else {
            self.stop(now, StopReason::CheckinDeclined);
        }
    }

    /// FR-008-14 / AC-008-05 (T-069): `detector://meeting` reported
    /// `meeting_ended` for the linked detection. Arms the 15 s "Continuar
    /// gravando" window via `toast://show`; `tick` stops the meeting when it
    /// lapses. A `detection_id` that is not this meeting's link — or no link
    /// at all (manual starts) — is a plain no-op, no toast.
    pub fn detection_ended(&mut self, detection_id: &str, now: Instant) {
        if self.status == Status::Done || !self.policy.auto_stop {
            return;
        }
        if self.detection_id.as_deref() != Some(detection_id) {
            return;
        }
        if self.auto_stop_since.is_some() {
            return; // already armed — the countdown is not re-extendable
        }
        self.auto_stop_since = Some(now);
        self.effects.push(Effect::Toast(ToastKind::AutoStop));
    }

    /// `meeting_continue_recording` — the "Continuar gravando" affordance
    /// (FR-008-14): cancels a pending auto-stop. Idempotent.
    pub fn continue_recording(&mut self) {
        self.auto_stop_since = None;
    }

    /// True while the 15 s auto-stop window is awaiting an answer.
    pub fn auto_stop_pending(&self) -> bool {
        self.auto_stop_since.is_some()
    }

    /// Periodic evaluation — the worker calls this whenever `next_deadline`
    /// lapses (and at least every `STATE_TICK` while recording).
    pub fn tick(&mut self, now: Instant) {
        if self.status == Status::Done {
            return;
        }
        if now >= self.tick_at {
            self.tick_at = now + STATE_TICK;
            if self.status == Status::Recording {
                self.push_event(now, "recording");
            }
        }

        // FR-009-08: the limit runs on wall-clock meeting duration — pause
        // does not extend it.
        if self.status == Status::Recording
            && !self.limit_warned
            && now + LIMIT_WARNING_LEAD >= self.limit_at
        {
            self.limit_warned = true;
            self.effects.push(Effect::Toast(ToastKind::LimitWarning));
        }
        if now >= self.limit_at {
            self.stop(now, StopReason::Limit);
            return;
        }

        // FR-008-14: the auto-stop grace runs on wall time — a paused
        // meeting whose linked call ended still stops (FR-009-06's frozen
        // silence clock does not apply here).
        if self
            .auto_stop_since
            .is_some_and(|since| now.saturating_duration_since(since) >= AUTO_STOP_GRACE)
        {
            self.stop(now, StopReason::MeetingEnded);
            return;
        }

        // FR-009-09: the silence clock freezes while paused, so neither the
        // check-in nor its answer window is evaluated in `Paused`.
        if self.status != Status::Recording {
            return;
        }
        match self.checkin_since {
            Some(since) if now.saturating_duration_since(since) >= CHECKIN_RESPONSE_WINDOW => {
                self.stop(now, StopReason::CheckinTimeout);
            }
            None if self.policy.silence_checkin && self.all_quiet(now) => {
                self.checkin_since = Some(now);
                self.effects.push(Effect::Toast(ToastKind::SilenceCheckin));
            }
            _ => {}
        }
    }

    /// Earliest instant the machine needs to be woken at — the worker's
    /// `recv_timeout` budget.
    pub fn next_deadline(&self) -> Option<Instant> {
        if self.status == Status::Done {
            return None;
        }
        let mut deadlines = vec![self.tick_at, self.limit_at];
        if let Some(since) = self.auto_stop_since {
            deadlines.push(since + AUTO_STOP_GRACE);
        }
        if !self.limit_warned {
            if let Some(warn_at) = self.limit_at.checked_sub(LIMIT_WARNING_LEAD) {
                deadlines.push(warn_at);
            }
        }
        match self.checkin_since {
            Some(since) => deadlines.push(since + CHECKIN_RESPONSE_WINDOW),
            None if self.policy.silence_checkin => {
                if let Some(quiet_since) = self.quiet_since() {
                    deadlines.push(quiet_since + SILENCE_CHECKIN_AFTER);
                }
            }
            _ => {}
        }
        deadlines.into_iter().min()
    }

    /// Milliseconds since the meeting started (pauses included).
    pub fn elapsed_ms(&self, now: Instant) -> u64 {
        self.ms(now).max(0) as u64
    }

    /// Current lifecycle status string (`meetings.status` values).
    pub fn status(&self) -> &'static str {
        self.status.as_str()
    }

    /// `true` while the meeting is `recording` or `paused`.
    pub fn is_active(&self) -> bool {
        self.status != Status::Done
    }

    /// True while a silence check-in toast awaits an answer.
    pub fn checkin_pending(&self) -> bool {
        self.checkin_since.is_some()
    }

    /// Journaled `meeting://state` payloads, in order.
    pub fn take_events(&mut self) -> Vec<MeetingStateEvent> {
        std::mem::take(&mut self.events)
    }

    /// Effects the worker still has to perform, in order.
    pub fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }

    /// Milliseconds from the meeting's monotonic base — segment timestamps.
    pub fn ms(&self, at: Instant) -> i64 {
        at.saturating_duration_since(self.t0).as_millis() as i64
    }

    /// When the *last* speech was heard across live tracks — the instant the
    /// current quiet streak began. `None` when no track is live.
    fn quiet_since(&self) -> Option<Instant> {
        self.last_speech.iter().flatten().copied().max()
    }

    /// FR-009-09: every live track silent for `SILENCE_CHECKIN_AFTER`.
    fn all_quiet(&self, now: Instant) -> bool {
        match self.quiet_since() {
            Some(since) => now.saturating_duration_since(since) >= SILENCE_CHECKIN_AFTER,
            // No live track at all — treat as quiet (mic dead in-person
            // meetings still get the "still there?" check).
            None => now.saturating_duration_since(self.t0) >= SILENCE_CHECKIN_AFTER,
        }
    }

    fn push_status_effect(&mut self, status: &'static str, error_code: Option<&'static str>) {
        self.effects
            .push(Effect::PersistStatus { status, error_code });
    }

    fn push_event(&mut self, now: Instant, status: &str) {
        self.events.push(MeetingStateEvent {
            meeting_id: self.meeting_id.clone(),
            status: status.to_string(),
            elapsed_ms: self.elapsed_ms(now),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: Duration = Duration::from_secs(60);

    fn policy() -> MeetingPolicy {
        MeetingPolicy {
            max_duration: 2 * 60 * MIN,
            silence_checkin: true,
            auto_stop: true,
        }
    }

    fn machine(t0: Instant) -> SessionMachine {
        SessionMachine::new("m-1".to_string(), t0, policy(), true, true, None)
    }

    /// A meeting linked to `detector://meeting` detection `d-1` — what every
    /// detector-initiated start (prompt or auto) produces (FR-008-14).
    fn detected_machine(t0: Instant) -> SessionMachine {
        SessionMachine::new(
            "m-1".to_string(),
            t0,
            policy(),
            true,
            true,
            Some("d-1".to_string()),
        )
    }

    fn effects(m: &mut SessionMachine) -> Vec<Effect> {
        m.take_effects()
    }

    #[test]
    fn limit_minutes_clamp_to_spec_options() {
        assert_eq!(clamp_meeting_max_minutes(0), 30);
        assert_eq!(clamp_meeting_max_minutes(31), 30);
        assert_eq!(clamp_meeting_max_minutes(45), 60);
        assert_eq!(clamp_meeting_max_minutes(120), 120);
        assert_eq!(clamp_meeting_max_minutes(999), 240);
    }

    #[test]
    fn starts_recording_with_indicator_and_state_event() {
        let t0 = Instant::now();
        let mut m = machine(t0);
        assert!(m.is_active());
        assert_eq!(m.status(), "recording");
        assert_eq!(effects(&mut m), vec![Effect::Indicator(true)]);
        let events = m.take_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].status, "recording");
        assert_eq!(events[0].elapsed_ms, 0);
    }

    #[test]
    fn stop_orders_capture_persist_processing_and_indicator() {
        let t0 = Instant::now();
        let mut m = machine(t0);
        m.stop(t0 + MIN, StopReason::User);
        let e = effects(&mut m);
        let position = |effect: &Effect| e.iter().position(|item| item == effect);
        assert!(
            position(&Effect::StopCapture)
                < position(&Effect::PersistStatus {
                    status: "processing",
                    error_code: None,
                })
                && position(&Effect::PersistStatus {
                    status: "processing",
                    error_code: None,
                }) < position(&Effect::RequestProcessing)
                && position(&Effect::RequestProcessing) < position(&Effect::Indicator(false))
        );
        assert_eq!(m.take_events().last().unwrap().status, "processing");
    }

    #[test]
    fn pause_writes_gap_marker_on_resume_and_restarts_capture() {
        let t0 = Instant::now();
        let mut m = machine(t0);
        m.pause(t0 + 10 * MIN);
        assert_eq!(m.status(), "paused");
        let e = effects(&mut m);
        assert!(e.contains(&Effect::StopCapture));
        assert!(e.contains(&Effect::PersistStatus {
            status: "paused",
            error_code: None
        }));
        assert_eq!(m.take_events().last().unwrap().status, "paused");

        m.resume(t0 + 12 * MIN);
        let e = effects(&mut m);
        assert!(e.contains(&Effect::WriteGapMarker {
            track: Track::Mic,
            start_ms: 600_000,
            end_ms: 720_000
        }));
        assert!(e.contains(&Effect::StartCapture));
        assert!(e.contains(&Effect::PersistStatus {
            status: "recording",
            error_code: None
        }));
        assert_eq!(m.status(), "recording");

        // Idempotent: a second pause/resume pair is rejected while stopped.
        m.pause(t0 + 12 * MIN);
        m.stop(t0 + 13 * MIN, StopReason::User);
        m.resume(t0 + 14 * MIN);
        assert!(!m.is_active());
    }

    #[test]
    fn pause_freezes_the_silence_clock() {
        let t0 = Instant::now();
        let mut m = machine(t0);
        // Speech at t0+5min, then silence; at t0+14min the 10-min clock has
        // 1 min left. Pause for 30 min — the clock must freeze.
        m.block_sealed(Track::Mic, true, t0 + 5 * MIN);
        m.pause(t0 + 14 * MIN);
        // While paused, ticks never fire the check-in.
        m.tick(t0 + 14 * MIN + SILENCE_CHECKIN_AFTER);
        m.tick(t0 + 40 * MIN);
        assert!(m.checkin_since.is_none());
        assert!(m.is_active());

        m.resume(t0 + 44 * MIN);
        // Still ~1 min of quiet left before the check-in.
        m.tick(t0 + 44 * MIN);
        assert!(m.checkin_since.is_none());
        m.tick(t0 + 45 * MIN + Duration::from_secs(1));
        assert!(m.checkin_since.is_some());
        assert!(effects(&mut m).contains(&Effect::Toast(ToastKind::SilenceCheckin)));
    }

    #[test]
    fn silence_checkin_times_out_and_stops() {
        let t0 = Instant::now();
        let mut m = machine(t0);
        // AC-009-08: 10 min of silence → check-in; 2 min unanswered → stop.
        m.tick(t0 + SILENCE_CHECKIN_AFTER + Duration::from_secs(1));
        assert!(m.checkin_since.is_some());
        m.tick(t0 + SILENCE_CHECKIN_AFTER + CHECKIN_RESPONSE_WINDOW + Duration::from_secs(2));
        assert!(!m.is_active());
        let e = effects(&mut m);
        assert!(e.contains(&Effect::PersistStatus {
            status: "processing",
            error_code: None
        }));
        assert!(e.contains(&Effect::RequestProcessing));
        assert!(e.contains(&Effect::Indicator(false)));
        assert_eq!(m.take_events().last().unwrap().status, "processing");
    }

    #[test]
    fn checkin_continue_restarts_the_silence_clock_and_decline_stops() {
        let t0 = Instant::now();
        let mut m = machine(t0);
        m.tick(t0 + SILENCE_CHECKIN_AFTER);
        assert!(m.checkin_since.is_some());
        m.checkin_respond(true, t0 + SILENCE_CHECKIN_AFTER + MIN);
        assert!(m.checkin_since.is_none());
        // "Continue" counts as presence — the next check-in is a full 10 min
        // away, not "the remainder of the old window".
        m.tick(t0 + SILENCE_CHECKIN_AFTER + MIN + SILENCE_CHECKIN_AFTER - Duration::from_secs(1));
        assert!(m.checkin_since.is_none());
        m.tick(t0 + SILENCE_CHECKIN_AFTER + MIN + SILENCE_CHECKIN_AFTER);
        assert!(m.checkin_since.is_some());
        m.checkin_respond(false, t0 + 25 * MIN);
        assert!(!m.is_active());
    }

    #[test]
    fn speech_on_either_track_defer_checkin() {
        let t0 = Instant::now();
        let mut m = machine(t0);
        // Only the system track speaks at 9min — mic silence alone is not
        // "both tracks quiet".
        m.block_sealed(Track::System, true, t0 + 9 * MIN);
        m.tick(t0 + SILENCE_CHECKIN_AFTER + Duration::from_secs(1));
        assert!(m.checkin_since.is_none());
        m.tick(t0 + 19 * MIN + Duration::from_secs(1));
        assert!(m.checkin_since.is_some());
    }

    #[test]
    fn checkin_disabled_by_policy() {
        let t0 = Instant::now();
        let mut m = SessionMachine::new(
            "m".to_string(),
            t0,
            MeetingPolicy {
                max_duration: 2 * 60 * MIN,
                silence_checkin: false,
                auto_stop: true,
            },
            true,
            true,
            None,
        );
        m.tick(t0 + SILENCE_CHECKIN_AFTER + Duration::from_secs(1));
        assert!(m.checkin_since.is_none());
        assert!(m.is_active());
    }

    #[test]
    fn duration_limit_warns_at_t_minus_5_extends_and_stops() {
        let t0 = Instant::now();
        let mut m = machine(t0);
        // 2 h limit → warning at 1h55m.
        m.tick(t0 + 115 * MIN);
        assert!(effects(&mut m).contains(&Effect::Toast(ToastKind::LimitWarning)));
        // Extend adds 30 min — deadline moves to 2h30m, warning re-arms.
        m.extend(t0 + 116 * MIN);
        m.tick(t0 + 146 * MIN);
        assert!(effects(&mut m).contains(&Effect::Toast(ToastKind::LimitWarning)));
        // New deadline reached → auto-stop into processing.
        m.tick(t0 + 150 * MIN);
        assert!(!m.is_active());
        assert_eq!(m.take_events().last().unwrap().status, "processing");
    }

    #[test]
    fn limit_applies_while_paused() {
        let t0 = Instant::now();
        let mut m = machine(t0);
        m.pause(t0 + MIN);
        m.tick(t0 + 2 * 60 * MIN);
        assert!(!m.is_active());
        assert!(effects(&mut m).contains(&Effect::StopCapture));
    }

    #[test]
    fn write_failure_burst_stops_with_error() {
        let t0 = Instant::now();
        let mut m = machine(t0);
        for _ in 0..WRITE_FAILURE_LIMIT - 1 {
            m.write_failed(t0 + MIN);
        }
        assert!(m.is_active());
        // A sealed block between failures resets the burst counter.
        m.block_sealed(Track::Mic, false, t0 + MIN);
        for _ in 0..WRITE_FAILURE_LIMIT {
            m.write_failed(t0 + MIN);
        }
        assert!(!m.is_active());
        assert!(effects(&mut m).contains(&Effect::PersistStatus {
            status: "error",
            error_code: Some("write_failed")
        }));
        // An errored meeting is not handed to the processor.
        assert!(!effects(&mut m).contains(&Effect::RequestProcessing));
        assert_eq!(m.take_events().last().unwrap().status, "error");
    }

    #[test]
    fn track_unavailable_toasts_and_drops_it_from_silence_tracking() {
        let t0 = Instant::now();
        let mut m = machine(t0);
        m.track_unavailable(Track::Mic);
        m.track_unavailable(Track::System);
        let e = effects(&mut m);
        assert!(e.contains(&Effect::Toast(ToastKind::MicUnavailable)));
        assert!(e.contains(&Effect::Toast(ToastKind::SystemUnavailable)));
        // With no live track the check-in still applies (t0 quiet streak).
        m.tick(t0 + SILENCE_CHECKIN_AFTER + Duration::from_secs(1));
        assert!(m.checkin_since.is_some());
    }

    #[test]
    fn device_detach_reattach_writes_system_gap_marker() {
        let t0 = Instant::now();
        let mut m = machine(t0);
        m.system_detached(t0 + 10 * MIN);
        m.system_attached(t0 + 10 * MIN + Duration::from_secs(2), None);
        assert!(effects(&mut m).contains(&Effect::WriteGapMarker {
            track: Track::System,
            start_ms: 600_000,
            end_ms: 602_000
        }));
        // The loopback's measured gap wins over the detach timestamp.
        m.system_detached(t0 + 20 * MIN);
        m.system_attached(
            t0 + 20 * MIN + Duration::from_secs(5),
            Some(Duration::from_secs(4)),
        );
        assert!(effects(&mut m).contains(&Effect::WriteGapMarker {
            track: Track::System,
            start_ms: 1_201_000,
            end_ms: 1_205_000
        }));
    }

    #[test]
    fn next_deadline_tracks_the_earliest_timer() {
        let t0 = Instant::now();
        let m = machine(t0);
        assert_eq!(m.next_deadline(), Some(t0 + STATE_TICK));
        let mut m = machine(t0);
        m.tick(t0 + SILENCE_CHECKIN_AFTER);
        assert_eq!(
            m.next_deadline(),
            Some(t0 + SILENCE_CHECKIN_AFTER + STATE_TICK)
        );
        m.stop(t0, StopReason::User);
        assert_eq!(m.next_deadline(), None);
    }

    // -- FR-008-14 auto-stop (T-069) ----------------------------------------

    #[test]
    fn detection_end_arms_auto_stop_and_lapse_stops() {
        let t0 = Instant::now();
        let mut m = detected_machine(t0);
        m.detection_ended("d-1", t0 + MIN);
        assert!(m.auto_stop_pending());
        assert!(effects(&mut m).contains(&Effect::Toast(ToastKind::AutoStop)));
        assert!(m.is_active());
        // 1 s before the grace lapses — still recording.
        m.tick(t0 + MIN + AUTO_STOP_GRACE - Duration::from_secs(1));
        assert!(m.is_active());
        // Grace lapsed → stop into processing like any graceful stop.
        m.tick(t0 + MIN + AUTO_STOP_GRACE);
        assert!(!m.is_active());
        let e = effects(&mut m);
        assert!(e.contains(&Effect::PersistStatus {
            status: "processing",
            error_code: None
        }));
        assert!(e.contains(&Effect::RequestProcessing));
        assert!(e.contains(&Effect::Indicator(false)));
        // The pending "finalizando em 15 s" notice must be closed with it.
        assert!(e.contains(&Effect::DismissToast(ToastKind::AutoStop)));
        assert_eq!(m.take_events().last().unwrap().status, "processing");
    }

    #[test]
    fn stale_checkin_toast_is_dismissed_on_timeout_and_speech() {
        // The "Ainda em reunião?" notice is exempt from the 60 s toast
        // collapse — without a backend dismissal it would linger with dead
        // buttons after the response window lapses or speech resumes.
        let t0 = Instant::now();
        let mut m = detected_machine(t0);
        m.tick(t0 + SILENCE_CHECKIN_AFTER);
        assert!(m.checkin_pending());

        // Timeout → stop: the check-in notice closes with the meeting.
        m.tick(t0 + SILENCE_CHECKIN_AFTER + CHECKIN_RESPONSE_WINDOW);
        assert!(!m.is_active());
        assert!(effects(&mut m).contains(&Effect::DismissToast(ToastKind::SilenceCheckin)));

        // Speech resuming clears a pending check-in the same way.
        let mut m2 = detected_machine(t0);
        m2.tick(t0 + SILENCE_CHECKIN_AFTER);
        assert!(m2.checkin_pending());
        m2.block_sealed(
            Track::Mic,
            true,
            t0 + SILENCE_CHECKIN_AFTER + Duration::from_secs(10),
        );
        assert!(!m2.checkin_pending());
        assert!(effects(&mut m2).contains(&Effect::DismissToast(ToastKind::SilenceCheckin)));

        // An explicit "Continuar" clears it too — even if a non-toast
        // surface answered.
        let mut m3 = detected_machine(t0);
        m3.tick(t0 + SILENCE_CHECKIN_AFTER);
        m3.checkin_respond(true, t0 + SILENCE_CHECKIN_AFTER + Duration::from_secs(20));
        assert!(effects(&mut m3).contains(&Effect::DismissToast(ToastKind::SilenceCheckin)));
    }

    #[test]
    fn detection_end_toast_carries_the_continue_recording_action() {
        let t0 = Instant::now();
        let mut m = detected_machine(t0);
        m.detection_ended("d-1", t0);
        assert_eq!(
            effects(&mut m).last(),
            Some(&Effect::Toast(ToastKind::AutoStop))
        );
        assert_eq!(ToastKind::AutoStop.kind(), "meeting_auto_stop");
        assert_eq!(ToastKind::AutoStop.action(), Some("continue_recording"));
    }

    #[test]
    fn detection_end_arms_a_wake_deadline_for_the_worker() {
        let t0 = Instant::now();
        let mut m = detected_machine(t0);
        m.detection_ended("d-1", t0 + MIN);
        // Tick to inside the last second of the grace: the 1 s state tick
        // would wake at t0+MIN+15.5s, so the lapse at +15s must win.
        m.tick(t0 + MIN + AUTO_STOP_GRACE - Duration::from_millis(500));
        let _ = effects(&mut m);
        assert!(m.is_active());
        assert_eq!(m.next_deadline(), Some(t0 + MIN + AUTO_STOP_GRACE));
    }

    #[test]
    fn continue_recording_cancels_the_pending_auto_stop() {
        let t0 = Instant::now();
        let mut m = detected_machine(t0);
        m.detection_ended("d-1", t0 + MIN);
        m.continue_recording();
        assert!(!m.auto_stop_pending());
        // Idempotent; meeting keeps recording well past the lapse.
        m.continue_recording();
        m.tick(t0 + MIN + AUTO_STOP_GRACE + Duration::from_secs(30));
        assert!(m.is_active());
        assert_eq!(m.status(), "recording");
    }

    #[test]
    fn auto_stop_only_arms_for_the_linked_detection() {
        let t0 = Instant::now();
        let mut m = detected_machine(t0);
        // A different detection ending is a no-op — no toast, no timer.
        m.detection_ended("d-OTHER", t0 + MIN);
        assert!(!m.auto_stop_pending());
        assert!(!effects(&mut m)
            .iter()
            .any(|e| matches!(e, Effect::Toast(_))));
        m.tick(t0 + MIN + AUTO_STOP_GRACE + MIN);
        assert!(m.is_active());
    }

    #[test]
    fn manual_meetings_never_auto_stop() {
        let t0 = Instant::now();
        let mut m = machine(t0); // detection_id: None
        m.detection_ended("d-1", t0 + MIN);
        assert!(!m.auto_stop_pending());
        assert!(!effects(&mut m)
            .iter()
            .any(|e| matches!(e, Effect::Toast(_))));
        m.tick(t0 + MIN + AUTO_STOP_GRACE + MIN);
        assert!(m.is_active());
    }

    #[test]
    fn auto_stop_setting_off_keeps_recording() {
        let t0 = Instant::now();
        let mut m = SessionMachine::new(
            "m".to_string(),
            t0,
            MeetingPolicy {
                auto_stop: false,
                ..policy()
            },
            true,
            true,
            Some("d-1".to_string()),
        );
        m.detection_ended("d-1", t0 + MIN);
        assert!(!m.auto_stop_pending());
        assert!(!effects(&mut m)
            .iter()
            .any(|e| matches!(e, Effect::Toast(_))));
        m.tick(t0 + MIN + AUTO_STOP_GRACE + MIN);
        assert!(m.is_active());
    }

    #[test]
    fn auto_stop_also_applies_while_paused() {
        let t0 = Instant::now();
        let mut m = detected_machine(t0);
        m.pause(t0 + MIN);
        m.detection_ended("d-1", t0 + 2 * MIN);
        assert!(m.auto_stop_pending());
        let _ = effects(&mut m);
        m.tick(t0 + 2 * MIN + AUTO_STOP_GRACE);
        assert!(!m.is_active());
    }

    #[test]
    fn detection_end_is_idempotent_and_noop_after_stop() {
        let t0 = Instant::now();
        let mut m = detected_machine(t0);
        m.detection_ended("d-1", t0 + MIN);
        assert!(effects(&mut m).contains(&Effect::Toast(ToastKind::AutoStop)));
        // A re-emitted end for the same detection does not re-arm or re-toast.
        m.detection_ended("d-1", t0 + 2 * MIN);
        assert!(!effects(&mut m)
            .iter()
            .any(|e| matches!(e, Effect::Toast(_))));
        m.tick(t0 + MIN + AUTO_STOP_GRACE);
        assert!(!m.is_active());
        // And after the stop the input is a full no-op.
        m.detection_ended("d-1", t0 + 3 * MIN);
        assert!(!effects(&mut m)
            .iter()
            .any(|e| matches!(e, Effect::Toast(_))));
    }
}
