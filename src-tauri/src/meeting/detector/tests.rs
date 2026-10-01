//! Scripted-timeline tests for the detector state machine (spec F008).
//! All times are `base() + N s` — no real sleeps.

use super::*;
use crate::meeting::classifier::RuleAction;
use std::sync::OnceLock;

const T0_MS: i64 = 1_700_000_000_000;

fn base() -> Instant {
    static T0: OnceLock<Instant> = OnceLock::new();
    *T0.get_or_init(Instant::now)
}

fn at(offset_secs: u64) -> Instant {
    base() + Duration::from_secs(offset_secs)
}

fn app(exe: &str, label: &str, pid: Option<u32>, action: RuleAction) -> MeetingApp {
    MeetingApp {
        label: label.to_string(),
        exe_name: exe.to_string(),
        exe_path: Some(format!("C:\\apps\\{exe}")),
        pid,
        action,
        since_ms: Some(T0_MS),
    }
}

fn app_since(
    exe: &str,
    label: &str,
    pid: Option<u32>,
    action: RuleAction,
    since_ms: i64,
) -> MeetingApp {
    let mut a = app(exe, label, pid, action);
    a.since_ms = Some(since_ms);
    a
}

fn usage(exe: &str) -> MicUsage {
    MicUsage {
        key: format!("C:\\apps\\{exe}"),
        exe_path: Some(format!("C:\\apps\\{exe}")),
        exe_name: exe.to_string(),
        since_ms: Some(T0_MS),
    }
}

fn usage_since(exe: &str, since_ms: i64) -> MicUsage {
    let mut u = usage(exe);
    u.since_ms = Some(since_ms);
    u
}

fn window(exe: &str, title: &str) -> WindowInfo {
    WindowInfo {
        pid: 42,
        exe_name: exe.to_string(),
        title: title.to_string(),
    }
}

fn rule(exe: &str, label: &str, pattern: Option<&str>, action: &str) -> MeetingAppRule {
    MeetingAppRule {
        id: format!("test-{label}-{exe}"),
        exe: exe.to_string(),
        title_pattern: pattern.map(str::to_string),
        label: label.to_string(),
        action: action.to_string(),
        builtin: true,
    }
}

fn meet_rule(exe: &str) -> MeetingAppRule {
    rule(
        exe,
        "Google Meet",
        Some(r"^Meet -|meet\.google\.com"),
        "ask",
    )
}

#[allow(clippy::too_many_arguments)]
fn tick_at(
    detector: &mut Detector,
    at: Instant,
    classified: &[MeetingApp],
    usages: &[MicUsage],
    windows: &[WindowInfo],
    rules: &[MeetingAppRule],
    any_call: bool,
) -> Vec<DetectorOutput> {
    detector.tick(&TickInput {
        classified,
        mic_usages: usages,
        windows,
        rules,
        detect_any_call: any_call,
        now: at,
        now_unix_ms: T0_MS + at.duration_since(base()).as_millis() as i64,
    })
}

fn detector() -> Detector {
    Detector::new("transcreve-ai.exe".to_string())
}

fn started_ids(outputs: &[DetectorOutput]) -> Vec<String> {
    outputs
        .iter()
        .filter_map(|o| match o {
            DetectorOutput::Started(d) => Some(d.detection_id.clone()),
            _ => None,
        })
        .collect()
}

fn ended_ids(outputs: &[DetectorOutput]) -> Vec<String> {
    outputs
        .iter()
        .filter_map(|o| match o {
            DetectorOutput::Ended { detection_id, .. } => Some(detection_id.clone()),
            _ => None,
        })
        .collect()
}

/// (detection_id, meeting_over) pairs — T-069's auto-stop keys on the flag.
fn ended_marks(outputs: &[DetectorOutput]) -> Vec<(String, bool)> {
    outputs
        .iter()
        .filter_map(|o| match o {
            DetectorOutput::Ended {
                detection_id,
                meeting_over,
            } => Some((detection_id.clone(), *meeting_over)),
            _ => None,
        })
        .collect()
}

/// Drive a Zoom detection to `live`; returns its `detection_id`.
fn detected_zoom(d: &mut Detector) -> String {
    let zoom = app("Zoom.exe", "Zoom", None, RuleAction::Ask);
    let usages = vec![usage("Zoom.exe")];
    let windows = vec![window("Zoom.exe", "Zoom")];
    let rules = vec![rule("Zoom.exe", "Zoom", None, "ask")];
    for s in 0..5 {
        tick_at(
            d,
            at(s),
            std::slice::from_ref(&zoom),
            &usages,
            &windows,
            &rules,
            false,
        );
    }
    let outputs = tick_at(d, at(5), &[zoom], &usages, &windows, &rules, false);
    started_ids(&outputs)
        .into_iter()
        .next()
        .expect("zoom must detect at t=5")
}

// -- Debounce (FR-008-02) ----------------------------------------------------

#[test]
fn detection_fires_only_after_five_stable_seconds() {
    let mut d = detector();
    let zoom = app("Zoom.exe", "Zoom", None, RuleAction::Ask);
    let usages = vec![usage("Zoom.exe")];
    let windows = vec![window("Zoom.exe", "Zoom")];
    let rules = vec![rule("Zoom.exe", "Zoom", None, "ask")];

    for s in 0..5 {
        let outputs = tick_at(
            &mut d,
            at(s),
            std::slice::from_ref(&zoom),
            &usages,
            &windows,
            &rules,
            false,
        );
        assert!(outputs.is_empty(), "tick {s}s must stay quiet: {outputs:?}");
    }
    let outputs = tick_at(
        &mut d,
        at(5),
        std::slice::from_ref(&zoom),
        &usages,
        &windows,
        &rules,
        false,
    );
    assert_eq!(started_ids(&outputs).len(), 1);
    // …and it never fires again while the meeting is steady.
    assert!(tick_at(
        &mut d,
        at(6),
        std::slice::from_ref(&zoom),
        &usages,
        &windows,
        &rules,
        false
    )
    .is_empty());
    assert!(tick_at(&mut d, at(60), &[zoom], &usages, &windows, &rules, false).is_empty());
}

#[test]
fn unstable_match_resets_the_debounce() {
    let mut d = detector();
    let zoom = app("Zoom.exe", "Zoom", None, RuleAction::Ask);
    let on = vec![zoom.clone()];
    let off: Vec<MeetingApp> = vec![];
    let usages = vec![usage("Zoom.exe")];
    let windows = vec![];
    let rules = vec![rule("Zoom.exe", "Zoom", None, "ask")];

    // Present 0..3s, gone at 4s, back at 5s: the clock restarts.
    for s in 0..4 {
        tick_at(&mut d, at(s), &on, &usages, &windows, &rules, false);
    }
    tick_at(&mut d, at(4), &off, &[], &windows, &rules, false);
    for s in 5..10 {
        let outputs = tick_at(&mut d, at(s), &on, &usages, &windows, &rules, false);
        assert!(outputs.is_empty(), "tick {s}s after gap must stay quiet");
    }
    let outputs = tick_at(&mut d, at(10), &on, &usages, &windows, &rules, false);
    assert_eq!(started_ids(&outputs).len(), 1);
}

// -- Title memory (FR-008-02 edge case) --------------------------------------

#[test]
fn browser_detection_survives_tab_switch_while_mic_held() {
    let mut d = detector();
    let meet = app("chrome.exe", "Google Meet", Some(42), RuleAction::Ask);
    let usages = vec![usage("chrome.exe")];
    let rules = vec![meet_rule("chrome.exe")];
    // Title matches at t=0 only; afterwards the user is on another tab.
    let meet_window = vec![window("chrome.exe", "Meet - abc-defg-hij")];
    let other_window = vec![window("chrome.exe", "YouTube - Google Chrome")];

    tick_at(&mut d, at(0), &[meet], &usages, &meet_window, &rules, false);
    // The match condition (current title OR a title hit within 10 min, mic
    // held) has been continuously true since t=0, so the 5 s debounce still
    // promotes at t=5 even though the title left at t=1.
    for s in 1..5 {
        let outputs = tick_at(&mut d, at(s), &[], &usages, &other_window, &rules, false);
        assert!(outputs.is_empty(), "memory keeps it pending, not dead");
    }
    let outputs = tick_at(&mut d, at(5), &[], &usages, &other_window, &rules, false);
    assert_eq!(
        started_ids(&outputs).len(),
        1,
        "memory (≤10 min, mic held) keeps presence past the debounce"
    );
    // Still live minutes later.
    assert!(tick_at(&mut d, at(300), &[], &usages, &other_window, &rules, false).is_empty());
}

#[test]
fn stale_title_memory_does_not_keep_a_released_browser_detection() {
    let mut d = detector();
    let meet = app("chrome.exe", "Google Meet", Some(42), RuleAction::Ask);
    let usages = vec![usage("chrome.exe")];
    let rules = vec![meet_rule("chrome.exe")];
    let other_window = vec![window("chrome.exe", "YouTube - Google Chrome")];

    // Title matched once at t=0; memory keeps the detection live while the
    // mic stays held — even past the 10 min window (FR-008-04 only ends on
    // release/exit, and the mic is the primary signal).
    for s in [0u64, 5, 700] {
        tick_at(
            &mut d,
            at(s),
            std::slice::from_ref(&meet),
            &usages,
            &other_window,
            &rules,
            false,
        );
    }
    // …but once the mic is released with no window matching the title, the
    // stale memory is gone and (c) ends it immediately.
    let outputs = tick_at(&mut d, at(701), &[], &[], &other_window, &rules, false);
    assert_eq!(ended_ids(&outputs).len(), 1);
}

// -- End of meeting (FR-008-04) ----------------------------------------------

#[test]
fn mic_release_for_fifteen_seconds_ends_the_meeting() {
    let mut d = detector();
    let id = detected_zoom(&mut d);
    let windows = vec![window("Zoom.exe", "Zoom")];
    let rules = vec![rule("Zoom.exe", "Zoom", None, "ask")];

    // Mic released at t=10; the app window still exists → grace runs.
    for s in 10..25 {
        let outputs = tick_at(&mut d, at(s), &[], &[], &windows, &rules, false);
        assert!(
            ended_ids(&outputs).is_empty(),
            "released < 15 s must not end (tick {s}s)"
        );
    }
    let outputs = tick_at(&mut d, at(25), &[], &[], &windows, &rules, false);
    assert_eq!(ended_ids(&outputs), vec![id]);
}

#[test]
fn reacquired_mic_inside_the_grace_keeps_the_meeting() {
    let mut d = detector();
    let zoom = app("Zoom.exe", "Zoom", None, RuleAction::Ask);
    let usages = vec![usage("Zoom.exe")];
    let windows = vec![window("Zoom.exe", "Zoom")];
    let rules = vec![rule("Zoom.exe", "Zoom", None, "ask")];
    detected_zoom(&mut d);

    // Released 10..17s, re-held at 18s — the pending end is cancelled.
    for s in 10..18 {
        tick_at(&mut d, at(s), &[], &[], &windows, &rules, false);
    }
    for s in 18..30 {
        let outputs = tick_at(
            &mut d,
            at(s),
            std::slice::from_ref(&zoom),
            &usages,
            &windows,
            &rules,
            false,
        );
        assert!(ended_ids(&outputs).is_empty());
    }
    // Released again at 30: a fresh 15 s window applies.
    for s in 30..44 {
        assert!(ended_ids(&tick_at(&mut d, at(s), &[], &[], &windows, &rules, false)).is_empty());
    }
    assert_eq!(
        ended_ids(&tick_at(&mut d, at(45), &[], &[], &windows, &rules, false)).len(),
        1
    );
}

#[test]
fn process_exit_ends_immediately_without_the_grace() {
    let mut d = detector();
    detected_zoom(&mut d);
    let rules = vec![rule("Zoom.exe", "Zoom", None, "ask")];

    // Process gone: no usage AND no window for the exe → end now (t=6 < 15 s).
    let outputs = tick_at(&mut d, at(6), &[], &[], &[], &rules, false);
    assert_eq!(ended_ids(&outputs).len(), 1);
}

#[test]
fn browser_ends_as_soon_as_title_gone_and_mic_released() {
    let mut d = detector();
    let meet = app("chrome.exe", "Google Meet", Some(42), RuleAction::Ask);
    let usages = vec![usage("chrome.exe")];
    let rules = vec![meet_rule("chrome.exe")];
    let meet_window = vec![window("chrome.exe", "Meet - abc-defg-hij")];

    for s in 0..=6 {
        tick_at(
            &mut d,
            at(s),
            std::slice::from_ref(&meet),
            &usages,
            &meet_window,
            &rules,
            false,
        );
    }

    // User left the call: mic released AND no window matches the Meet title —
    // Chrome itself is still around → FR-008-04(c) ends now, not in 15 s.
    let outputs = tick_at(
        &mut d,
        at(7),
        &[],
        &[],
        &[window("chrome.exe", "New Tab")],
        &rules,
        false,
    );
    assert_eq!(ended_ids(&outputs).len(), 1);
}

#[test]
fn browser_mic_release_with_title_still_matching_uses_the_grace() {
    let mut d = detector();
    let meet = app("chrome.exe", "Google Meet", Some(42), RuleAction::Ask);
    let usages = vec![usage("chrome.exe")];
    let rules = vec![meet_rule("chrome.exe")];
    let meet_window = vec![window("chrome.exe", "Meet - abc-defg-hij")];

    for s in 0..=6 {
        tick_at(
            &mut d,
            at(s),
            std::slice::from_ref(&meet),
            &usages,
            &meet_window,
            &rules,
            false,
        );
    }
    // Mic released at t=7 but the Meet window still matches the title → (c)
    // does not fire; the (a) 15 s grace runs instead.
    for s in 7..22 {
        assert!(
            ended_ids(&tick_at(
                &mut d,
                at(s),
                &[],
                &[],
                &meet_window,
                &rules,
                false
            ))
            .is_empty(),
            "grace must run while a Meet window still matches ({s}s)"
        );
    }
    assert_eq!(
        ended_ids(&tick_at(
            &mut d,
            at(22),
            &[],
            &[],
            &meet_window,
            &rules,
            false
        ))
        .len(),
        1
    );
}

// -- detection_id / dedup (FR-008-05, AC-008-07) -----------------------------

#[test]
fn new_mic_acquisition_after_the_end_is_a_new_detection() {
    let mut d = detector();
    let windows = vec![window("Zoom.exe", "Zoom")];
    let rules = vec![rule("Zoom.exe", "Zoom", None, "ask")];
    let first = detected_zoom(&mut d);

    // End it: the process exits.
    tick_at(&mut d, at(6), &[], &[], &[], &rules, false);

    // New meeting: same exe, new since_ms → new detection_id, fires again.
    let later_ms = T0_MS + 90_000;
    let usages2 = vec![usage_since("Zoom.exe", later_ms)];
    let zoom2 = app_since("Zoom.exe", "Zoom", None, RuleAction::Ask, later_ms);
    for s in 10..15 {
        assert!(tick_at(
            &mut d,
            at(s),
            std::slice::from_ref(&zoom2),
            &usages2,
            &windows,
            &rules,
            false
        )
        .is_empty());
    }
    let outputs = tick_at(&mut d, at(15), &[zoom2], &usages2, &windows, &rules, false);
    assert_eq!(started_ids(&outputs).len(), 1);
    assert_ne!(
        started_ids(&outputs)[0],
        first,
        "a new meeting gets a new id"
    );
}

#[test]
fn dismissed_detection_never_refires_for_the_same_meeting() {
    let mut d = detector();
    let zoom = app("Zoom.exe", "Zoom", None, RuleAction::Ask);
    let usages = vec![usage("Zoom.exe")];
    let windows = vec![window("Zoom.exe", "Zoom")];
    let rules = vec![rule("Zoom.exe", "Zoom", None, "ask")];
    let id = detected_zoom(&mut d);

    assert!(d.dismiss(&id).is_some());
    assert!(
        d.dismiss(&id).is_some(),
        "dismiss is idempotent on a live detection"
    );

    // The meeting ends silently: no Ended output for a dismissed id.
    let outputs = tick_at(&mut d, at(60), &[], &[], &[], &rules, false);
    assert!(
        outputs.is_empty(),
        "dismissed detection must not emit Ended"
    );

    // A gate reset (pause/unpause) drops tracking but keeps the dedup set.
    assert!(d.reset().is_empty());
    for s in 100..105 {
        assert!(tick_at(
            &mut d,
            at(s),
            std::slice::from_ref(&zoom),
            &usages,
            &windows,
            &rules,
            false
        )
        .is_empty());
    }
    let outputs = tick_at(&mut d, at(105), &[zoom], &usages, &windows, &rules, false);
    assert!(
        outputs.is_empty(),
        "dismissed detection_id must not re-fire after unpause"
    );
}

// -- Ignore / actions ---------------------------------------------------------

#[test]
fn ignore_matches_never_become_candidates() {
    let mut d = detector();
    let discord = app("Discord.exe", "Discord", None, RuleAction::Ignore);
    let usages = vec![usage("Discord.exe")];
    let windows = vec![window("Discord.exe", "call")];
    let rules = vec![rule("Discord.exe", "Discord", None, "ignore")];
    for s in 0..30 {
        let outputs = tick_at(
            &mut d,
            at(s),
            std::slice::from_ref(&discord),
            &usages,
            &windows,
            &rules,
            false,
        );
        assert!(outputs.is_empty(), "AC-008-03: ignore must stay silent");
    }
}

#[test]
fn auto_start_action_rides_on_the_detection() {
    let mut d = detector();
    let zoom = app("Zoom.exe", "Zoom", None, RuleAction::AutoStart);
    let usages = vec![usage("Zoom.exe")];
    let windows = vec![window("Zoom.exe", "Zoom")];
    let rules = vec![rule("Zoom.exe", "Zoom", None, "auto_start")];
    for s in 0..5 {
        tick_at(
            &mut d,
            at(s),
            std::slice::from_ref(&zoom),
            &usages,
            &windows,
            &rules,
            false,
        );
    }
    let outputs = tick_at(&mut d, at(5), &[zoom], &usages, &windows, &rules, false);
    match &outputs[..] {
        [DetectorOutput::Started(det)] => assert_eq!(det.action, RuleAction::AutoStart),
        other => panic!("expected one Started, got {other:?}"),
    }
}

// -- FR-008-03 "detect any call" ----------------------------------------------

#[test]
fn any_call_fires_after_ten_seconds_with_exe_stem_label() {
    let mut d = detector();
    let usages = vec![usage("audacity.exe")];
    let windows = vec![window("audacity.exe", "Audacity")];
    for s in 0..10 {
        assert!(
            tick_at(&mut d, at(s), &[], &usages, &windows, &[], true).is_empty(),
            "any-call must wait 10 s (tick {s}s)"
        );
    }
    let outputs = tick_at(&mut d, at(10), &[], &usages, &windows, &[], true);
    match &outputs[..] {
        [DetectorOutput::Started(det)] => {
            assert_eq!(det.app_label, "audacity");
            assert_eq!(det.source, DetectionSource::AnyCall);
            assert_eq!(det.action, RuleAction::Ask);
        }
        other => panic!("expected one any-call Started, got {other:?}"),
    }
}

#[test]
fn any_call_excludes_browsers_self_and_ignored_apps() {
    let mut d = detector();
    let usages = vec![
        usage("chrome.exe"),
        usage("transcreve-ai.exe"),
        usage("Discord.exe"),
    ];
    let discord_ignore = app("Discord.exe", "Discord", None, RuleAction::Ignore);
    let rules = vec![rule("Discord.exe", "Discord", None, "ignore")];
    let windows = vec![];
    for s in 0..30 {
        let outputs = tick_at(
            &mut d,
            at(s),
            std::slice::from_ref(&discord_ignore),
            &usages,
            &windows,
            &rules,
            true,
        );
        assert!(
            outputs.is_empty(),
            "no any-call detection at {s}s: {outputs:?}"
        );
    }
}

#[test]
fn any_call_does_not_double_report_a_rule_matched_app() {
    let mut d = detector();
    let zoom = app("Zoom.exe", "Zoom", None, RuleAction::Ask);
    let usages = vec![usage("Zoom.exe")];
    let windows = vec![window("Zoom.exe", "Zoom")];
    let rules = vec![rule("Zoom.exe", "Zoom", None, "ask")];
    for s in 0..=10 {
        tick_at(
            &mut d,
            at(s),
            std::slice::from_ref(&zoom),
            &usages,
            &windows,
            &rules,
            true,
        );
    }
    // One rule detection exists; Zoom must not also appear as any-call.
    let outs = tick_at(&mut d, at(11), &[zoom], &usages, &windows, &rules, true);
    assert!(outs.is_empty());
}

#[test]
fn disabling_any_call_ends_its_live_detection() {
    let mut d = detector();
    let usages = vec![usage("audacity.exe")];
    let windows = vec![];
    for s in 0..=10 {
        tick_at(&mut d, at(s), &[], &usages, &windows, &[], true);
    }
    let outputs = tick_at(&mut d, at(11), &[], &usages, &windows, &[], false);
    assert_eq!(ended_ids(&outputs).len(), 1);
}

#[test]
fn only_real_meeting_ends_carry_meeting_over() {
    // T-069 / FR-008-14: the auto-stop keys on `meeting_over`. A real
    // FR-008-04 end flags it; a lifecycle end (detector reset on
    // pause/offline, `detect_any_call` toggle-off) must not — pausing
    // detection mid-meeting can never kill a recording.
    let mut d = detector();
    let id = detected_zoom(&mut d);
    let windows = vec![window("Zoom.exe", "Zoom")];
    let rules = vec![rule("Zoom.exe", "Zoom", None, "ask")];
    for s in 10..25 {
        tick_at(&mut d, at(s), &[], &[], &windows, &rules, false);
    }
    let outputs = tick_at(&mut d, at(25), &[], &[], &windows, &rules, false);
    assert_eq!(ended_marks(&outputs), vec![(id, true)]);

    // Suppression reset → same detection closed, but meeting_over: false.
    let mut d = detector();
    let id = detected_zoom(&mut d);
    let outputs = d.reset();
    assert_eq!(ended_marks(&outputs), vec![(id, false)]);

    // Any-call toggle-off → lifecycle end, meeting_over: false.
    let mut d = detector();
    let usages = vec![usage("audacity.exe")];
    for s in 0..=10 {
        tick_at(&mut d, at(s), &[], &usages, &[], &[], true);
    }
    let outputs = tick_at(&mut d, at(11), &[], &usages, &[], &[], false);
    assert_eq!(ended_marks(&outputs).len(), 1);
    assert!(ended_marks(&outputs).iter().all(|(_, over)| !over));
}

// -- respond bookkeeping ------------------------------------------------------

#[test]
fn unknown_detection_id_is_not_found() {
    let mut d = detector();
    assert!(d.find("nope").is_none());
    assert!(d.dismiss("nope").is_none());
}

#[test]
fn reset_ends_live_detections_and_clears_pending() {
    let mut d = detector();
    let zoom = app("Zoom.exe", "Zoom", None, RuleAction::Ask);
    let usages = vec![usage("Zoom.exe")];
    let windows = vec![window("Zoom.exe", "Zoom")];
    let rules = vec![rule("Zoom.exe", "Zoom", None, "ask")];
    let id = detected_zoom(&mut d);

    let outputs = d.reset();
    assert_eq!(ended_ids(&outputs), vec![id]);
    // A still-running meeting re-detects from scratch after the reset.
    for s in 10..15 {
        assert!(tick_at(
            &mut d,
            at(s),
            std::slice::from_ref(&zoom),
            &usages,
            &windows,
            &rules,
            false
        )
        .is_empty());
    }
    assert_eq!(
        started_ids(&tick_at(
            &mut d,
            at(15),
            &[zoom],
            &usages,
            &windows,
            &rules,
            false
        ))
        .len(),
        1
    );
}

#[test]
fn payload_shapes_match_the_contract() {
    let d = Detection {
        detection_id: "zoom.exe:1:2".to_string(),
        app_label: "Zoom".to_string(),
        exe_name: "Zoom.exe".to_string(),
        exe_path: Some("C:\\apps\\Zoom.exe".to_string()),
        pid: None,
        action: RuleAction::Ask,
        started_at_ms: T0_MS,
        source: DetectionSource::Rule,
    };
    let start = serde_json::to_value(DetectorMeetingEvent::started(&d)).unwrap();
    assert_eq!(
        start,
        serde_json::json!({
            "detection_id": "zoom.exe:1:2",
            "app_label": "Zoom",
            "exe": "Zoom.exe",
            "action": "ask",
            "started_at": T0_MS,
        })
    );
    let end = serde_json::to_value(DetectorMeetingEvent::ended("x", true)).unwrap();
    assert_eq!(
        end,
        serde_json::json!({"detection_id": "x", "ended": true, "meeting_ended": true})
    );
    // A lifecycle end (reset / toggle-off) is flagged so T-069's auto-stop
    // ignores it; dismissal serializes the same way.
    let end = serde_json::to_value(DetectorMeetingEvent::ended("x", false)).unwrap();
    assert_eq!(
        end,
        serde_json::json!({"detection_id": "x", "ended": true, "meeting_ended": false})
    );
    let dis = serde_json::to_value(DetectorMeetingEvent::dismissed("x")).unwrap();
    assert_eq!(
        dis,
        serde_json::json!({"detection_id": "x", "ended": true, "meeting_ended": false, "dismissed": true})
    );
}

#[test]
fn auto_start_is_rule_or_global() {
    // FR-008-13: `auto_start` rules always auto-start; `ask` rules only when
    // the global toggle is on. `ignore` never reaches promotion.
    assert!(wants_auto_start(RuleAction::AutoStart, false));
    assert!(wants_auto_start(RuleAction::AutoStart, true));
    assert!(!wants_auto_start(RuleAction::Ask, false));
    assert!(wants_auto_start(RuleAction::Ask, true));
    assert!(!wants_auto_start(RuleAction::Ignore, false));
    // The global toggle must not resurrect an `ignore` verdict — Ignore
    // candidates are dropped upstream anyway (belt-and-braces).
    assert!(!wants_auto_start(RuleAction::Ignore, true));
}
