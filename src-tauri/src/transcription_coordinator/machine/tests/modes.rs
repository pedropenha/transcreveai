use super::*;

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
