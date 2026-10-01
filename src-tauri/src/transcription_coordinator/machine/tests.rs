use super::*;

const BINDING: &str = "transcribe";
const OTHER_BINDING: &str = "transcribe_with_post_process";
const HOLD_THRESHOLD: Duration = Duration::from_millis(300);

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn ptt_input(is_pressed: bool) -> InputEvent {
    InputEvent {
        binding_id: BINDING.to_string(),
        hotkey_string: BINDING.to_string(),
        is_pressed,
        mode: ShortcutActivation::PushToTalk,
        hold_threshold: Duration::ZERO,
        external: false,
        policy: SessionPolicy::default(),
    }
}

fn toggle_input(external: bool) -> InputEvent {
    toggle_input_for(BINDING, external)
}

fn toggle_input_for(binding_id: &str, external: bool) -> InputEvent {
    InputEvent {
        binding_id: binding_id.to_string(),
        hotkey_string: binding_id.to_string(),
        is_pressed: true,
        mode: ShortcutActivation::Toggle,
        hold_threshold: Duration::ZERO,
        external,
        policy: SessionPolicy::default(),
    }
}

fn input(mode: ShortcutActivation, is_pressed: bool) -> InputEvent {
    InputEvent {
        binding_id: BINDING.to_string(),
        hotkey_string: BINDING.to_string(),
        is_pressed,
        mode,
        hold_threshold: HOLD_THRESHOLD,
        external: false,
        policy: SessionPolicy::default(),
    }
}

/// Report the pipeline as done and let the `Done` dwell elapse so the machine
/// settles back to `Idle`.
fn finish_and_settle(state: &mut CoordinatorState, now: Instant) -> Option<Effect> {
    let effect = state.on_pipeline_finished(PipelineOutcome::Done, now);
    if state.stage.is_dwelling() {
        state.on_deadline(now + DONE_DWELL + ms(1));
    }
    effect
}

/// Drive one full session: press, commit past the arming window, then
/// stop into the busy pipeline.
fn drive_into_busy(state: &mut CoordinatorState, t0: Instant) {
    let mode = ShortcutActivation::HoldOrToggle;
    assert!(matches!(
        state.on_input(input(mode, true), t0),
        Some(Effect::Start { .. })
    ));
    assert!(state.on_input(input(mode, false), t0 + ms(800)).is_none());
    assert!(matches!(
        state.on_grace_expired(),
        Some(Effect::Stop { .. })
    ));
    assert!(state.stage.is_busy());
}

#[test]
fn push_to_talk_release_while_recording_defers_release() {
    assert_eq!(
        classify_ptt_event(None, false, true, "transcribe", Some("transcribe")),
        PttAction::DeferRelease
    );
}

#[test]
fn push_to_talk_press_matching_pending_release_cancels_release() {
    assert_eq!(
        classify_ptt_event(
            Some("transcribe"),
            true,
            true,
            "transcribe",
            Some("transcribe")
        ),
        PttAction::CancelRelease
    );
}

#[test]
fn toggle_mode_press_and_release_pass_through() {
    assert_eq!(
        classify_ptt_event(
            Some("transcribe"),
            true,
            false,
            "transcribe",
            Some("transcribe")
        ),
        PttAction::Passthrough
    );
    assert_eq!(
        classify_ptt_event(None, false, false, "transcribe", Some("transcribe")),
        PttAction::Passthrough
    );
}

#[test]
fn press_for_different_binding_than_pending_release_passes_through() {
    assert_eq!(
        classify_ptt_event(
            Some("transcribe"),
            true,
            true,
            "transcribe_with_post_process",
            Some("transcribe")
        ),
        PttAction::Passthrough
    );
}

#[test]
fn press_matching_pending_release_cancels_without_recording_state() {
    assert_eq!(
        classify_ptt_event(Some("transcribe"), true, true, "transcribe", None),
        PttAction::CancelRelease
    );
}

// ---------------------------------------------------------------------
// Busy-pipeline input classification.
//
// Toggle-style triggers (SIGUSR2, CLI flags, pedals that signal on both
// edges) flip state on every edge. Dropping a press that arrives while
// the previous pipeline is still processing desyncs the parity: the next
// edge then starts a recording no one will stop, leaving the overlay
// waiting for input with the button long released.
// ---------------------------------------------------------------------

#[test]
fn toggle_press_during_processing_remembers_start() {
    assert_eq!(
        classify_busy_input(true, ShortcutActivation::Toggle, None),
        BusyAction::Remember
    );
}

#[test]
fn second_toggle_press_during_processing_forgets_press() {
    assert_eq!(
        classify_busy_input(true, ShortcutActivation::Toggle, Some(Remembered::Locked)),
        BusyAction::Forget
    );
}

#[test]
fn toggle_release_during_processing_is_ignored() {
    assert_eq!(
        classify_busy_input(false, ShortcutActivation::Toggle, None),
        BusyAction::Ignore
    );
    assert_eq!(
        classify_busy_input(false, ShortcutActivation::Toggle, Some(Remembered::Locked)),
        BusyAction::Ignore
    );
}

#[test]
fn hold_modes_classify_busy_inputs_by_pending_state() {
    let cases = [
        (true, None, BusyAction::Remember),
        (true, Some(Remembered::Held), BusyAction::Ignore),
        (true, Some(Remembered::Locked), BusyAction::Forget),
        (false, None, BusyAction::Ignore),
        (false, Some(Remembered::Held), BusyAction::Ignore),
        (false, Some(Remembered::Locked), BusyAction::Ignore),
    ];

    for mode in [
        ShortcutActivation::PushToTalk,
        ShortcutActivation::HoldOrToggle,
    ] {
        for (is_pressed, remembered, expected) in cases {
            assert_eq!(classify_busy_input(is_pressed, mode, remembered), expected);
        }
    }
}

/// Toggle parity across a busy window: an odd number of presses remembers
/// one start, each further press flips the remembered press off/on again.
#[test]
fn toggle_presses_alternate_remember_and_forget_while_busy() {
    let mut remembered = None;
    for expected in [
        BusyAction::Remember,
        BusyAction::Forget,
        BusyAction::Remember,
    ] {
        let action = classify_busy_input(true, ShortcutActivation::Toggle, remembered);
        assert_eq!(action, expected);
        remembered = (action == BusyAction::Remember).then_some(Remembered::Locked);
    }
    assert!(remembered.is_some());
}

// ---------------------------------------------------------------------
// Sequence-level regression coverage for issue #1539.
//
// Under X11 key auto-repeat, holding a push-to-talk key does not emit one
// long press. It emits the initial press followed by a stream of
// synthesized release/press pairs, then a single genuine release on key-up.
// Before the fix, every synthesized release passed straight through and
// stopped recording, so holding the key "rapidly toggled" recording on and
// off. The fix defers each release for a short grace window and cancels it
// when the matching auto-repeat press arrives.
//
// The harness below drives the real `CoordinatorState` through whole event
// sequences — the same `on_input` / `on_deadline` handlers the coordinator
// thread runs — so a burst can be exercised deterministically without a
// Tauri AppHandle or real timers, and the tests can never drift from the
// production transitions.
// ---------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Ev {
    /// A key-down event (real initial press or a synthesized auto-repeat press).
    Press,
    /// A key-up event (synthesized auto-repeat release or the genuine key-up).
    Release,
}

struct DriveResult {
    starts: u32,
    stops: u32,
    discards: u32,
    stage_name: &'static str,
}

/// Feeds an event sequence to a real [`CoordinatorState`] the way the
/// coordinator thread would; effects are counted instead of executed.
fn drive(events: &[Ev]) -> DriveResult {
    let mut state = CoordinatorState::new();
    let mut clock = Instant::now();
    let mut starts = 0u32;
    let mut stops = 0u32;
    let mut discards = 0u32;

    for ev in events {
        // Auto-repeat events arrive a few ms apart, well inside DEBOUNCE.
        clock += Duration::from_millis(5);

        let effect = state.on_input(ptt_input(matches!(ev, Ev::Press)), clock);
        match effect {
            Some(Effect::Start { .. }) => starts += 1,
            Some(Effect::Stop { .. }) => stops += 1,
            Some(Effect::Discard { .. }) => discards += 1,
            None => {}
        }
    }

    DriveResult {
        starts,
        stops,
        discards,
        stage_name: state.stage.name(),
    }
}

/// Initial press plus several synthesized release/press pairs, as X11 emits
/// while a push-to-talk key is held down.
fn autorepeat_burst() -> Vec<Ev> {
    let mut events = vec![Ev::Press];
    for _ in 0..6 {
        events.push(Ev::Release);
        events.push(Ev::Press);
    }
    events
}

/// Regression for #1539: a burst of X11 auto-repeat release/press pairs must
/// not stop recording. Before the fix the first synthesized release stopped
/// recording immediately (stops == 1, stage left Recording), which produced
/// the rapid on/off toggling. With the fix the releases are coalesced and
/// capture stays continuously active for the whole burst.
#[test]
fn x11_autorepeat_burst_does_not_toggle_recording() {
    let result = drive(&autorepeat_burst());
    assert_eq!(result.starts, 1, "recording should start exactly once");
    assert_eq!(
        result.stops, 0,
        "synthesized auto-repeat releases must not stop recording mid-burst"
    );
    assert!(
        result.discards == 0 && (result.stage_name == "arming" || result.stage_name == "recording"),
        "capture must remain active across the entire auto-repeat burst"
    );
}

/// Complements the burst test: a *held* key that is genuinely released stops
/// exactly once. Here the burst lasts past the arming window (the commit
/// deadline fires), then the key-up ends a real hold.
#[test]
fn genuine_release_after_grace_stops_recording_once() {
    let mut state = CoordinatorState::new();
    let t0 = Instant::now();

    assert!(matches!(
        state.on_input(ptt_input(true), t0),
        Some(Effect::Start { .. })
    ));
    // Hold past the arming window so this is a committed hold, not a tap.
    state.on_deadline(t0 + ARMING_WINDOW);
    assert!(matches!(state.stage, Stage::Recording(_)));

    assert!(state
        .on_input(ptt_input(false), t0 + ARMING_WINDOW + ms(500))
        .is_none());
    assert!(
        matches!(state.on_grace_expired(), Some(Effect::Stop { .. })),
        "a genuine release should stop recording exactly once"
    );
    assert!(matches!(state.stage, Stage::Transcribing(_)));
}

// ---------------------------------------------------------------------
// Sequence-level coverage of the busy-pipeline and cancel paths, driven
// through the real machine.
// ---------------------------------------------------------------------

/// PTT press while the pipeline is busy is remembered and starts recording
/// once the pipeline drains.
#[test]
fn press_during_processing_starts_after_drain() {
    let mut state = CoordinatorState::new();
    let now = Instant::now();

    let effect = state.on_input(ptt_input(true), now);
    assert!(matches!(effect, Some(Effect::Start { .. })));
    // Commit the session so the hold release below is a real hold.
    state.on_deadline(now + ARMING_WINDOW);

    let effect = state.on_input(ptt_input(false), now + ARMING_WINDOW + ms(100));
    assert!(effect.is_none(), "release should be deferred, not fired");

    let effect = state.on_grace_expired();
    assert!(matches!(effect, Some(Effect::Stop { .. })));
    assert!(state.stage.is_busy());

    let effect = state.on_input(ptt_input(true), now + ARMING_WINDOW + ms(200));
    assert!(effect.is_none(), "busy pipeline must remember, not start");
    assert_eq!(state.pending.len(), 1);

    let effect = state.on_pipeline_finished(PipelineOutcome::Done, now + ms(1000));
    assert!(
        matches!(effect, Some(Effect::Start { .. })),
        "remembered press should start once the pipeline drains"
    );
}

/// Two toggle presses inside one busy window net to no-op: nothing starts
/// when the pipeline drains (toggle parity).
#[test]
fn toggle_presses_during_processing_net_noop_after_drain() {
    let mut state = CoordinatorState::new();
    let now = Instant::now();

    let effect = state.on_input(ptt_input(true), now);
    assert!(matches!(effect, Some(Effect::Start { .. })));
    state.on_deadline(now + ARMING_WINDOW);
    let effect = state.on_input(ptt_input(false), now + ARMING_WINDOW + ms(100));
    assert!(effect.is_none());
    let effect = state.on_grace_expired();
    assert!(matches!(effect, Some(Effect::Stop { .. })));

    let toggle = |state: &mut CoordinatorState, at: Instant| state.on_input(toggle_input(true), at);

    let effect = toggle(&mut state, now + ms(2000));
    assert!(effect.is_none());
    let effect = toggle(&mut state, now + ms(2100));
    assert!(effect.is_none());

    let effect = state.on_pipeline_finished(PipelineOutcome::Done, now + ms(3000));
    assert!(
        effect.is_none(),
        "even number of busy toggle presses must not start recording"
    );
    assert!(matches!(state.stage, Stage::Done(..)));
}

/// Cancel while processing abandons a remembered press: the pipeline drains
/// to idle and nothing starts.
#[test]
fn cancel_during_processing_drops_remembered_press() {
    let mut state = CoordinatorState::new();
    let now = Instant::now();

    let effect = state.on_input(ptt_input(true), now);
    assert!(matches!(effect, Some(Effect::Start { .. })));
    state.on_deadline(now + ARMING_WINDOW);
    let effect = state.on_input(ptt_input(false), now + ARMING_WINDOW + ms(100));
    assert!(effect.is_none());
    let effect = state.on_grace_expired();
    assert!(matches!(effect, Some(Effect::Stop { .. })));

    let effect = state.on_input(ptt_input(true), now + ms(2000));
    assert!(effect.is_none());

    state.on_cancel(false);
    assert!(
        state.stage.is_busy(),
        "cancel must not reset mid-processing — the pipeline still finishes"
    );

    let effect = state.on_pipeline_finished(PipelineOutcome::Done, now + ms(3000));
    assert!(
        effect.is_none(),
        "cancelled session must not spawn a deferred recording"
    );
    assert!(matches!(state.stage, Stage::Done(..)));
    state.on_deadline(now + ms(3000) + DONE_DWELL + ms(1));
    assert!(matches!(state.stage, Stage::Idle));
}

/// With the FIFO queue, a press for a different binding while busy queues a
/// second session rather than being dropped: pending sessions still start in
/// order (FR-002-16), and the same binding's parity is preserved.
#[test]
fn busy_presses_queue_fifotically_across_bindings() {
    let mut state = CoordinatorState::new();
    let now = Instant::now();
    drive_into_busy(&mut state, now);

    let at = |ms| now + Duration::from_millis(ms);
    assert!(state.on_input(toggle_input(false), at(200)).is_none());
    assert!(state
        .on_input(toggle_input_for(OTHER_BINDING, false), at(300))
        .is_none());
    assert_eq!(state.pending.len(), 2, "both presses queue in FIFO order");

    // First drain starts the oldest queued session.
    match state.on_pipeline_finished(PipelineOutcome::Done, at(400)) {
        Some(Effect::Start { binding_id, .. }) => assert_eq!(binding_id, BINDING),
        other => panic!("expected Start for '{BINDING}', got {other:?}"),
    }
    assert_eq!(state.pending.len(), 1);
}

/// Toggle parity applies per binding: two toggles for one binding inside the
/// busy window net to no-op while a third binding's press still queues.
#[test]
fn toggle_parity_is_per_binding_in_the_queue() {
    let mut state = CoordinatorState::new();
    let now = Instant::now();
    drive_into_busy(&mut state, now);

    let at = |ms| now + Duration::from_millis(ms);
    assert!(state.on_input(toggle_input(false), at(200)).is_none());
    assert!(state
        .on_input(toggle_input_for(OTHER_BINDING, false), at(300))
        .is_none());
    assert!(state.on_input(toggle_input(false), at(400)).is_none());

    assert_eq!(state.pending.len(), 1);
    match state.on_pipeline_finished(PipelineOutcome::Done, at(500)) {
        Some(Effect::Start { binding_id, .. }) => {
            assert_eq!(binding_id, OTHER_BINDING)
        }
        other => panic!("expected Start for '{OTHER_BINDING}', got {other:?}"),
    }
}

/// The queue is bounded (FR-002-16, `session_queue_size`): once saturated,
/// further presses are rejected and the Flow Bar gets a `queue_full` notice.
#[test]
fn queue_saturates_at_policy_capacity() {
    let mut state = CoordinatorState::new();
    let now = Instant::now();
    drive_into_busy(&mut state, now);
    state.take_events();

    let policy = SessionPolicy {
        queue_capacity: 2,
        ..SessionPolicy::default()
    };
    let press = |b: &str| InputEvent {
        binding_id: b.to_string(),
        hotkey_string: b.to_string(),
        is_pressed: true,
        mode: ShortcutActivation::Toggle,
        hold_threshold: Duration::ZERO,
        external: false,
        policy,
    };

    state.on_input(press(BINDING), now + ms(200));
    state.on_input(press(OTHER_BINDING), now + ms(300));
    state.on_input(press(BINDING), now + ms(400));
    // Capacity is 2: the third press is rejected (the first two toggles of
    // BINDING net to one queued entry? no — toggle parity: BINDING appears
    // twice → net removes it, so the queue holds only OTHER_BINDING).
    assert_eq!(
        state.pending.len(),
        1,
        "toggle parity removes the doubled binding entry"
    );

    // Fill the queue for real, then verify rejection.
    let mut s2 = CoordinatorState::new();
    drive_into_busy(&mut s2, now);
    s2.take_events();
    s2.on_input(press(BINDING), now + ms(200));
    s2.on_input(press(OTHER_BINDING), now + ms(300));
    assert_eq!(s2.pending.len(), 2);
    s2.on_input(press("cancel-test-binding"), now + ms(400));
    assert_eq!(s2.pending.len(), 2, "queue must reject past capacity");
    let events = s2.take_events();
    assert!(
        events
            .iter()
            .any(|e| e.notice.as_deref() == Some("session_queue_full")),
        "saturation should emit a session_queue_full notice"
    );
}

/// External triggers fire on every edge by design (e.g. SIGUSR2 sent on
/// both key press and release). Two edges inside the debounce window must
/// both be honoured, or the parity desyncs and recording wedges on.
#[test]
fn external_edges_inside_debounce_window_are_not_dropped() {
    let mut state = CoordinatorState::new();
    let now = Instant::now();

    let effect = state.on_input(toggle_input(true), now);
    assert!(matches!(effect, Some(Effect::Start { .. })));

    let effect = state.on_input(toggle_input(true), now + Duration::from_millis(5));
    assert!(
        matches!(effect, Some(Effect::Stop { .. })),
        "second external edge inside DEBOUNCE must stop the session"
    );
    assert!(state.stage.is_busy());
}

/// Physical keyboard presses keep the debounce: a repeat inside the window
/// is still dropped and recording stays active.
#[test]
fn keyboard_press_inside_debounce_window_is_still_dropped() {
    let mut state = CoordinatorState::new();
    let now = Instant::now();

    let effect = state.on_input(ptt_input(true), now);
    assert!(matches!(effect, Some(Effect::Start { .. })));

    let effect = state.on_input(ptt_input(true), now + Duration::from_millis(5));
    assert!(
        effect.is_none(),
        "keyboard repeat inside DEBOUNCE must be debounced"
    );
    assert!(state.stage.is_capturing());
}

/// If the start effect fails to begin recording (e.g. microphone access
/// denied), the optimistic transition becomes a brief `Error` (with the
/// `start_failed` code) that settles back to `Idle`.
#[test]
fn failed_start_reports_error_then_idles() {
    let mut state = CoordinatorState::new();
    let now = Instant::now();

    let effect = state.on_input(ptt_input(true), now);
    assert!(matches!(effect, Some(Effect::Start { .. })));

    state.on_start_result(BINDING, false, now);
    assert!(matches!(state.stage, Stage::Error(..)));
    let events = state.take_events();
    assert!(events
        .iter()
        .any(|e| e.state == "error" && e.error.as_deref() == Some("start_failed")));

    state.on_deadline(now + ERROR_DWELL + ms(1));
    assert!(matches!(state.stage, Stage::Idle));
}

// ---------------------------------------------------------------------
// Arming: capture starts on press (FR-002-10) but the session only commits
// to Recording after the arming window or a lock event.
// ---------------------------------------------------------------------

#[test]
fn press_enters_arming_then_commits_to_recording() {
    let mut state = CoordinatorState::new();
    let t0 = Instant::now();

    assert!(matches!(
        state.on_input(ptt_input(true), t0),
        Some(Effect::Start { .. })
    ));
    assert!(matches!(state.stage, Stage::Arming(_)));

    assert!(state.on_deadline(t0 + ARMING_WINDOW).is_none());
    assert!(matches!(state.stage, Stage::Recording(_)));

    let states: Vec<&'static str> = state.take_events().iter().map(|e| e.state).collect();
    assert_eq!(states, ["arming", "recording"]);
}

/// AC-002-02: a ~100 ms push-to-talk press released without a second tap
/// produces no transcription at all — the session is discarded in Arming.
#[test]
fn ptt_tap_without_second_tap_is_discarded() {
    let mut state = CoordinatorState::new();
    let t0 = Instant::now();

    assert!(matches!(
        state.on_input(ptt_input(true), t0),
        Some(Effect::Start { .. })
    ));
    assert!(state.on_input(ptt_input(false), t0 + ms(100)).is_none());
    assert!(state.on_grace_expired().is_none(), "tap must not stop");

    // Still capturing, waiting out the double-tap window.
    assert!(matches!(state.stage, Stage::Arming(_)));

    let effect = state.on_deadline(t0 + ms(100) + DOUBLE_TAP_WINDOW);
    assert!(matches!(effect, Some(Effect::Discard { .. })));
    assert!(matches!(state.stage, Stage::Idle));
    assert_eq!(
        state
            .take_events()
            .iter()
            .map(|e| e.state)
            .collect::<Vec<_>>(),
        ["arming", "idle"]
    );
}

/// FR-002-07 / AC-002-03: a second tap inside the window locks the session
/// hands-free; a later press ends it.
#[test]
fn ptt_double_tap_locks_hands_free() {
    let mut state = CoordinatorState::new();
    let t0 = Instant::now();

    state.on_input(ptt_input(true), t0);
    state.on_input(ptt_input(false), t0 + ms(100));
    assert!(state.on_grace_expired().is_none());

    // Second tap inside the 350 ms window.
    let effect = state.on_input(ptt_input(true), t0 + ms(300));
    assert!(effect.is_none());
    assert!(matches!(state.stage, Stage::Recording(_)));
    assert!(state.is_locked());

    // Its release is ignored; a later press stops.
    assert!(state.on_input(ptt_input(false), t0 + ms(400)).is_none());
    assert!(matches!(
        state.on_input(ptt_input(true), t0 + ms(5000)),
        Some(Effect::Stop { .. })
    ));
    assert!(matches!(state.stage, Stage::Transcribing(_)));
}

/// With double-tap disabled, a PTT tap discards immediately.
#[test]
fn ptt_tap_discards_immediately_when_double_tap_disabled() {
    let mut state = CoordinatorState::new();
    let t0 = Instant::now();
    let mut tap = ptt_input(true);
    tap.policy.double_tap = false;
    let mut release = ptt_input(false);
    release.policy.double_tap = false;

    assert!(matches!(
        state.on_input(tap, t0),
        Some(Effect::Start { .. })
    ));
    assert!(state.on_input(release, t0 + ms(100)).is_none());
    assert!(
        matches!(state.on_grace_expired(), Some(Effect::Discard { .. })),
        "no double-tap wait when the feature is off"
    );
    assert!(matches!(state.stage, Stage::Idle));
}

/// During the second-tap window the session still captures — a stray release
/// for the same binding must not resolve as a hold.
#[test]
fn release_during_second_tap_window_is_ignored() {
    let mut state = CoordinatorState::new();
    let t0 = Instant::now();

    state.on_input(ptt_input(true), t0);
    state.on_input(ptt_input(false), t0 + ms(100));
    state.on_grace_expired();

    assert!(state.on_input(ptt_input(false), t0 + ms(150)).is_none());
    assert!(
        state.pending_release.is_none(),
        "no release may be deferred"
    );
    assert!(matches!(state.stage, Stage::Arming(_)));
}

// ---------------------------------------------------------------------
// Duration limit (FR-002-13, AC-002-11): T-60s warning, graceful stop.
// ---------------------------------------------------------------------

#[test]
fn duration_warning_fires_sixty_seconds_before_limit() {
    let mut state = CoordinatorState::new();
    let t0 = Instant::now();
    let mut press = ptt_input(true);
    press.policy.max_duration = Duration::from_secs(5 * 60);

    state.on_input(press, t0);
    state.on_deadline(t0 + ARMING_WINDOW);
    state.take_events();

    state.on_deadline(t0 + Duration::from_secs(240));
    let events = state.take_events();
    assert!(
        events
            .iter()
            .any(|e| e.notice.as_deref() == Some("duration_warning")),
        "T-60s warning must fire"
    );
    assert!(
        state.stage.is_capturing(),
        "warning must not stop recording"
    );
}

#[test]
fn duration_limit_stops_and_processes_normally() {
    let mut state = CoordinatorState::new();
    let t0 = Instant::now();
    let mut press = toggle_input(false);
    press.policy.max_duration = Duration::from_secs(60);

    assert!(matches!(
        state.on_input(press, t0),
        Some(Effect::Start { .. })
    ));
    state.on_deadline(t0 + ARMING_WINDOW);

    let effect = state.on_deadline(t0 + Duration::from_secs(60));
    assert!(
        matches!(effect, Some(Effect::Stop { .. })),
        "hitting the limit must end capture gracefully (AC-002-11)"
    );
    assert!(matches!(state.stage, Stage::Transcribing(_)));
    let events = state.take_events();
    assert!(events.iter().any(|e| e.state == "transcribing"));
}

/// A 1-minute limit warns immediately — T-60 s is the start of the session.
#[test]
fn one_minute_limit_warns_immediately() {
    let mut state = CoordinatorState::new();
    let t0 = Instant::now();
    let mut press = ptt_input(true);
    press.policy.max_duration = Duration::from_secs(60);

    state.on_input(press, t0);
    state.on_deadline(t0 + ms(10));
    let events = state.take_events();
    assert!(
        events
            .iter()
            .any(|e| e.notice.as_deref() == Some("duration_warning")),
        "with a 1 min limit the T-60s warning fires at once"
    );
}

// ---------------------------------------------------------------------
// Pipeline phases and outcomes: Transcribing → Processing → Inserting →
// Done | Error, then Idle.
// ---------------------------------------------------------------------

#[test]
fn pipeline_phases_advance_the_state() {
    let mut state = CoordinatorState::new();
    let t0 = Instant::now();
    drive_into_busy(&mut state, t0);
    state.take_events();

    state.on_pipeline_phase(PipelinePhase::Processing);
    assert!(matches!(state.stage, Stage::Processing(_)));
    state.on_pipeline_phase(PipelinePhase::Inserting);
    assert!(matches!(state.stage, Stage::Inserting(_)));

    assert!(state
        .on_pipeline_finished(PipelineOutcome::Done, t0 + ms(5000))
        .is_none());
    assert!(matches!(state.stage, Stage::Done(..)));

    state.on_deadline(t0 + ms(5000) + DONE_DWELL + ms(1));
    assert!(matches!(state.stage, Stage::Idle));

    let states: Vec<&'static str> = state.take_events().iter().map(|e| e.state).collect();
    assert_eq!(states, ["processing", "inserting", "done", "idle"]);
}

#[test]
fn pipeline_failure_enters_error_then_idles() {
    let mut state = CoordinatorState::new();
    let t0 = Instant::now();
    drive_into_busy(&mut state, t0);
    state.take_events();

    assert!(state
        .on_pipeline_finished(
            PipelineOutcome::Failed("stt unavailable".to_string()),
            t0 + ms(3000)
        )
        .is_none());
    assert!(matches!(state.stage, Stage::Error(..)));
    let events = state.take_events();
    assert!(events
        .iter()
        .any(|e| e.state == "error" && e.error.as_deref() == Some("stt unavailable")));

    state.on_deadline(t0 + ms(3000) + ERROR_DWELL + ms(1));
    assert!(matches!(state.stage, Stage::Idle));
}

#[test]
fn empty_pipeline_outcome_emits_nothing_heard() {
    let mut state = CoordinatorState::new();
    let t0 = Instant::now();
    drive_into_busy(&mut state, t0);
    state.take_events();

    state.on_pipeline_finished(PipelineOutcome::Empty, t0 + ms(3000));
    assert!(matches!(state.stage, Stage::Idle));
    let events = state.take_events();
    assert!(events
        .iter()
        .any(|e| e.state == "idle" && e.notice.as_deref() == Some("nothing_heard")));
}

/// A press during the Done dwell starts a fresh session — the pipeline is
/// already free, the dwell is only for display.
#[test]
fn press_during_done_dwell_starts_a_new_session() {
    let mut state = CoordinatorState::new();
    let t0 = Instant::now();
    drive_into_busy(&mut state, t0);
    assert!(state
        .on_pipeline_finished(PipelineOutcome::Done, t0 + ms(2000))
        .is_none());
    assert!(matches!(state.stage, Stage::Done(..)));

    assert!(matches!(
        state.on_input(toggle_input(false), t0 + ms(2100)),
        Some(Effect::Start { .. })
    ));
    assert!(matches!(state.stage, Stage::Arming(_)));
}

// ---------------------------------------------------------------------
// Hold-or-toggle (the combined mode from #147) and the two legacy modes,
// driven through the real machine on a synthetic clock.
// ---------------------------------------------------------------------

/// Hold-or-toggle: a key held past the threshold is push-to-talk — the
/// (deferred) release stops recording.
#[test]
fn hold_or_toggle_long_hold_stops_on_release() {
    let mode = ShortcutActivation::HoldOrToggle;
    let mut state = CoordinatorState::new();
    let t0 = Instant::now();

    assert!(matches!(
        state.on_input(input(mode, true), t0),
        Some(Effect::Start { .. })
    ));
    assert!(state.on_input(input(mode, false), t0 + ms(800)).is_none());
    assert!(
        matches!(state.on_grace_expired(), Some(Effect::Stop { .. })),
        "an 800ms hold must stop when its release grace elapses"
    );
    assert!(matches!(state.stage, Stage::Transcribing(_)));
}

/// Hold-or-toggle: a tap keeps recording (locked on); the next press stops.
#[test]
fn hold_or_toggle_tap_locks_recording_until_next_press() {
    let mode = ShortcutActivation::HoldOrToggle;
    let mut state = CoordinatorState::new();
    let t0 = Instant::now();

    assert!(matches!(
        state.on_input(input(mode, true), t0),
        Some(Effect::Start { .. })
    ));
    assert!(state.on_input(input(mode, false), t0 + ms(120)).is_none());
    assert!(
        state.on_grace_expired().is_none(),
        "a 120ms tap must not stop recording"
    );
    assert!(matches!(state.stage, Stage::Recording(_)));
    assert!(state.is_locked());

    // Seconds later the user presses again to finish.
    assert!(matches!(
        state.on_input(input(mode, true), t0 + ms(5000)),
        Some(Effect::Stop { .. })
    ));
    assert!(matches!(state.stage, Stage::Transcribing(_)));
    // The release of that stopping press lands in the busy window and is
    // ignored, so nothing is remembered for the drain.
    assert!(state.on_input(input(mode, false), t0 + ms(5080)).is_none());
    assert!(finish_and_settle(&mut state, t0 + ms(6000)).is_none());
    assert!(matches!(state.stage, Stage::Idle));
}

/// Hold-or-toggle: a locked session ignores stray releases — only a press
/// ends it.
#[test]
fn hold_or_toggle_locked_session_ignores_release() {
    let mode = ShortcutActivation::HoldOrToggle;
    let mut state = CoordinatorState::new();
    let t0 = Instant::now();

    state.on_input(input(mode, true), t0);
    state.on_input(input(mode, false), t0 + ms(100));
    assert!(state.on_grace_expired().is_none());
    assert!(state.is_locked());

    assert!(state.on_input(input(mode, false), t0 + ms(900)).is_none());
    assert!(
        state.pending_release.is_none(),
        "no release may be deferred once locked"
    );
    assert!(matches!(state.stage, Stage::Recording(_)));
}

/// Hold-or-toggle: while the key is genuinely held, extra presses do not
/// stop the recording (that is the release's job).
#[test]
fn hold_or_toggle_press_while_held_is_ignored() {
    let mode = ShortcutActivation::HoldOrToggle;
    let mut state = CoordinatorState::new();
    let t0 = Instant::now();

    state.on_input(input(mode, true), t0);
    state.on_deadline(t0 + ARMING_WINDOW);
    assert!(state.on_input(input(mode, true), t0 + ms(400)).is_none());
    assert!(matches!(state.stage, Stage::Recording(_)));
    assert!(!state.is_locked());
}

/// Hold-or-toggle under X11 auto-repeat: the synthesized release/press
/// pairs must not be misread as taps. The hold is measured from the
/// original key-down to the genuine key-up.
#[test]
fn hold_or_toggle_autorepeat_burst_is_one_long_hold() {
    let mode = ShortcutActivation::HoldOrToggle;
    let mut state = CoordinatorState::new();
    let t0 = Instant::now();
    let mut clock = t0;

    assert!(matches!(
        state.on_input(input(mode, true), clock),
        Some(Effect::Start { .. })
    ));
    // ~600ms of auto-repeat pairs a few ms apart.
    for _ in 0..60 {
        clock += ms(5);
        assert!(state.on_input(input(mode, false), clock).is_none());
        clock += ms(5);
        assert!(state.on_input(input(mode, true), clock).is_none());
        assert!(
            state.pending_release.is_none(),
            "auto-repeat press must cancel the deferred release"
        );
    }
    assert!(!state.is_locked(), "no tap may be classified mid-burst");

    clock += ms(5);
    assert!(state.on_input(input(mode, false), clock).is_none());
    assert!(
        matches!(state.on_grace_expired(), Some(Effect::Stop { .. })),
        "the genuine release after a ~600ms hold must stop recording"
    );
}

/// Hold-or-toggle: a press remembered during the busy window is measured
/// from the real key-down, so a hold that straddles the drain still counts
/// as a hold when it is released shortly after recording actually starts.
#[test]
fn hold_or_toggle_remembered_press_measures_hold_from_real_key_down() {
    let mode = ShortcutActivation::HoldOrToggle;
    let mut state = CoordinatorState::new();
    let t0 = Instant::now();

    // Previous session: hold, release, stop → busy.
    drive_into_busy(&mut state, t0);

    // Pressed again while busy; still held when the pipeline drains 700ms later.
    assert!(state.on_input(input(mode, true), t0 + ms(1000)).is_none());
    assert!(matches!(
        state.on_pipeline_finished(PipelineOutcome::Done, t0 + ms(1700)),
        Some(Effect::Start { .. })
    ));
    // Released 100ms after recording began — but 800ms after key-down.
    assert!(state.on_input(input(mode, false), t0 + ms(1800)).is_none());
    assert!(
        matches!(state.on_grace_expired(), Some(Effect::Stop { .. })),
        "held 800ms overall: must stop, not lock"
    );
}

/// Toggle: releases never stop, the next press does. (Toggle is the
/// combined machine with the session locked from the start.)
#[test]
fn toggle_mode_ignores_release_and_stops_on_next_press() {
    let mode = ShortcutActivation::Toggle;
    let mut state = CoordinatorState::new();
    let t0 = Instant::now();

    assert!(matches!(
        state.on_input(input(mode, true), t0),
        Some(Effect::Start { .. })
    ));
    assert!(state.is_locked());
    assert!(state.on_input(input(mode, false), t0 + ms(100)).is_none());
    assert!(
        state.pending_release.is_none(),
        "toggle never defers releases"
    );
    assert!(state.on_input(input(mode, false), t0 + ms(3000)).is_none());
    assert!(matches!(
        state.on_input(input(mode, true), t0 + ms(4000)),
        Some(Effect::Stop { .. })
    ));
}

/// Push-to-talk: a hold past the arming window stops on release — there is
/// no tap-to-lock in this mode (the tap boundary is the arming window).
#[test]
fn push_to_talk_hold_stops_on_release() {
    let mode = ShortcutActivation::PushToTalk;
    let mut state = CoordinatorState::new();
    let t0 = Instant::now();

    assert!(matches!(
        state.on_input(input(mode, true), t0),
        Some(Effect::Start { .. })
    ));
    // Held past the arming window.
    state.on_deadline(t0 + ARMING_WINDOW);
    assert!(state.on_input(input(mode, false), t0 + ms(400)).is_none());
    assert!(matches!(
        state.on_grace_expired(),
        Some(Effect::Stop { .. })
    ));
}

/// Cancel (Escape) during a locked hold-or-toggle session resets cleanly so
/// the next press starts a fresh recording rather than stopping a dead one.
#[test]
fn hold_or_toggle_cancel_clears_locked_session() {
    let mode = ShortcutActivation::HoldOrToggle;
    let mut state = CoordinatorState::new();
    let t0 = Instant::now();

    state.on_input(input(mode, true), t0);
    state.on_input(input(mode, false), t0 + ms(100));
    assert!(state.on_grace_expired().is_none());
    assert!(state.is_locked());

    state.on_cancel(true);
    assert!(matches!(state.stage, Stage::Idle));
    assert!(!state.is_locked());
    assert!(matches!(
        state.on_input(input(mode, true), t0 + ms(2000)),
        Some(Effect::Start { .. })
    ));
}

/// Switching to toggle while an unlocked hold recording is running must not
/// strand it: in toggle mode a press always stops.
#[test]
fn toggle_press_stops_recording_started_as_hold() {
    let mut state = CoordinatorState::new();
    let t0 = Instant::now();

    state.on_input(input(ShortcutActivation::HoldOrToggle, true), t0);
    assert!(!state.is_locked());
    assert!(matches!(
        state.on_input(input(ShortcutActivation::Toggle, true), t0 + ms(2000)),
        Some(Effect::Stop { .. })
    ));
}

// Hold-vs-tap classification while the previous transcription is busy.

#[test]
fn hold_or_toggle_tap_during_processing_queues_locked_start() {
    let mode = ShortcutActivation::HoldOrToggle;
    let mut state = CoordinatorState::new();
    let t0 = Instant::now();
    drive_into_busy(&mut state, t0);

    assert!(state.on_input(input(mode, true), t0 + ms(1000)).is_none());
    assert!(state.on_input(input(mode, false), t0 + ms(1100)).is_none());
    assert!(state.on_grace_expired().is_none());
    assert!(state.is_locked(), "a busy tap should queue a locked start");
    assert!(matches!(
        state.on_pipeline_finished(PipelineOutcome::Done, t0 + ms(2000)),
        Some(Effect::Start { .. })
    ));
    assert!(state.is_locked());
}

#[test]
fn hold_or_toggle_completed_hold_during_processing_nets_noop() {
    let mode = ShortcutActivation::HoldOrToggle;
    let mut state = CoordinatorState::new();
    let t0 = Instant::now();
    drive_into_busy(&mut state, t0);

    assert!(state.on_input(input(mode, true), t0 + ms(1000)).is_none());
    assert!(state.on_input(input(mode, false), t0 + ms(1600)).is_none());
    assert!(state.on_grace_expired().is_none());
    assert!(!state.is_locked());

    assert!(
        state
            .on_pipeline_finished(PipelineOutcome::Done, t0 + ms(3000))
            .is_none(),
        "a 600ms hold that ended before the drain has nothing left to start"
    );
    assert!(matches!(state.stage, Stage::Done(..)));
}

#[test]
fn hold_or_toggle_two_taps_during_processing_net_noop() {
    let mode = ShortcutActivation::HoldOrToggle;
    let mut state = CoordinatorState::new();
    let t0 = Instant::now();
    drive_into_busy(&mut state, t0);

    assert!(state.on_input(input(mode, true), t0 + ms(1000)).is_none());
    assert!(state.on_input(input(mode, false), t0 + ms(1100)).is_none());
    assert!(state.on_grace_expired().is_none());
    assert!(state.is_locked());

    assert!(state.on_input(input(mode, true), t0 + ms(1500)).is_none());
    assert!(
        !state.is_locked(),
        "the second tap's press forgets the queued tap"
    );
    assert!(state.on_input(input(mode, false), t0 + ms(1600)).is_none());
    assert!(state.pending_release.is_none());

    assert!(state
        .on_pipeline_finished(PipelineOutcome::Done, t0 + ms(3000))
        .is_none());
    assert!(matches!(state.stage, Stage::Done(..)));
}

#[test]
fn ptt_tap_inside_busy_window_nets_noop() {
    let mut state = CoordinatorState::new();
    let t0 = Instant::now();
    drive_into_busy(&mut state, t0);

    assert!(state.on_input(ptt_input(true), t0 + ms(1000)).is_none());
    assert!(state.on_input(ptt_input(false), t0 + ms(1040)).is_none());
    assert!(state.on_grace_expired().is_none());
    assert!(state
        .on_pipeline_finished(PipelineOutcome::Done, t0 + ms(3000))
        .is_none());
}

/// The pipeline drains inside the 50ms grace of a busy tap: recording
/// starts first (unlocked, from the real key-down), and the grace then
/// resolves against the live session, locking it as the tap it was.
#[test]
fn hold_or_toggle_drain_inside_busy_release_grace_still_classifies_tap() {
    let mode = ShortcutActivation::HoldOrToggle;
    let mut state = CoordinatorState::new();
    let t0 = Instant::now();
    drive_into_busy(&mut state, t0);

    assert!(state.on_input(input(mode, true), t0 + ms(1000)).is_none());
    assert!(state.on_input(input(mode, false), t0 + ms(1100)).is_none());
    assert!(matches!(
        state.on_pipeline_finished(PipelineOutcome::Done, t0 + ms(1110)),
        Some(Effect::Start { .. })
    ));
    assert!(!state.is_locked());

    assert!(state.on_grace_expired().is_none());
    assert!(state.is_locked(), "the deferred 100ms release is a tap");
}

/// X11 auto-repeat while busy, key still held at the drain: recording
/// starts measured from the first press, not from the last synthesized
/// press before the drain. Released 400ms after the real key-down but
/// only ~100ms after the drain — a hold, so it must stop rather than lock.
#[test]
fn hold_or_toggle_autorepeat_burst_straddling_drain_measures_from_first_press() {
    let mode = ShortcutActivation::HoldOrToggle;
    let mut state = CoordinatorState::new();
    let t0 = Instant::now();
    drive_into_busy(&mut state, t0);

    let mut clock = t0 + ms(1000);
    assert!(state.on_input(input(mode, true), clock).is_none());
    for _ in 0..30 {
        clock += ms(5);
        assert!(state.on_input(input(mode, false), clock).is_none());
        clock += ms(5);
        assert!(state.on_input(input(mode, true), clock).is_none());
        assert!(state.pending_release.is_none());
    }

    // Drain at ~t0 + 1300ms with the key still down.
    assert!(matches!(
        state.on_pipeline_finished(PipelineOutcome::Done, clock),
        Some(Effect::Start { .. })
    ));
    assert!(!state.is_locked());

    for _ in 0..10 {
        clock += ms(5);
        assert!(state.on_input(input(mode, false), clock).is_none());
        clock += ms(5);
        assert!(state.on_input(input(mode, true), clock).is_none());
    }
    assert_eq!(clock, t0 + ms(1400));
    assert!(state.on_input(input(mode, false), clock).is_none());
    assert!(
        matches!(state.on_grace_expired(), Some(Effect::Stop { .. })),
        "held 400ms since the real key-down: must stop, not lock"
    );
    assert!(matches!(state.stage, Stage::Transcribing(_)));
}
