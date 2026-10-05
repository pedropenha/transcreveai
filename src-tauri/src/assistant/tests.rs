use super::panel::*;
use super::state::*;
use super::turn::*;

use super::*;
use crate::settings::{AppSettings, AssistantPanelPosition, CliAgentConfig};

fn msg(role: &str, content: impl Into<String>) -> AssistantMessage {
    AssistantMessage {
        role: role.to_string(),
        content: content.into(),
    }
}

fn provider(id: &str) -> PostProcessProvider {
    PostProcessProvider {
        id: id.to_string(),
        label: id.to_string(),
        base_url: format!("https://example.com/{id}"),
        allow_base_url_edit: true,
        models_endpoint: Some("/models".to_string()),
        supports_structured_output: false,
    }
}

#[test]
fn truncate_history_keeps_the_newest_messages() {
    let mut messages: Vec<AssistantMessage> = (0..60)
        .map(|i| {
            msg(
                if i % 2 == 0 { "user" } else { "assistant" },
                format!("m{i}"),
            )
        })
        .collect();
    truncate_history(&mut messages);
    assert_eq!(messages.len(), DEFAULT_HISTORY_LIMIT);
    assert_eq!(messages.last().unwrap().content, "m59");
    assert_eq!(messages.first().unwrap().content, "m20");
}

#[test]
fn truncate_history_never_leaves_an_assistant_head() {
    // A length-cut tail can open on a dangling assistant answer —
    // providers (Anthropic among them) reject assistant-first histories.
    let mut messages: Vec<AssistantMessage> = (0..=60)
        .map(|i| {
            msg(
                if i % 2 == 0 { "user" } else { "assistant" },
                format!("m{i}"),
            )
        })
        .collect();
    truncate_history(&mut messages);
    assert_eq!(messages.first().unwrap().role, "user");
    assert_eq!(messages.last().unwrap().content, "m60");
}

#[test]
fn truncate_history_enforces_the_char_budget() {
    // Six ~30k-char turns (180k total) exceed MAX_INPUT_CHARS — the
    // newest pairs are kept, oldest dropped whole. The context block
    // reserve (~24k) counts against the budget too: with 30k messages the
    // retained tail can shrink to the single newest prompt, which `len >
    // 1` keeps — so size alone isn't a stable assertion; the budget and
    // the user-head invariant are.
    let mut messages: Vec<AssistantMessage> = (0..6)
        .map(|i| {
            msg(
                if i % 2 == 0 { "user" } else { "assistant" },
                "x".repeat(10_000),
            )
        })
        .collect();
    truncate_history(&mut messages);
    assert!(
        SYSTEM_PROMPT.chars().count() + CONTEXT_RESERVE_CHARS + history_chars(&messages)
            <= MAX_INPUT_CHARS
    );
    assert_eq!(messages.first().unwrap().role, "user");
    // 60k of messages don't fit next to the reserve — the tail is bounded.
    assert!(messages.len() < 6);
    // A single oversized newest prompt is never dropped — `len > 1`
    // stops the budget loop.
    let mut lone = vec![msg("user", "x".repeat(MAX_INPUT_CHARS * 2))];
    truncate_history(&mut lone);
    assert_eq!(lone.len(), 1);
}

#[test]
fn provider_status_flags_experimental_cli_adapters() {
    // Experimental adapters are explicitly selectable for the assistant:
    // detected → ready with the `CliAgentExperimental` advisory hint;
    // undetected → `CliAgentNotDetected`. They are never silently ready —
    // the hint always says why.
    let settings = AppSettings::default();
    for experimental in ["cli_agent/cursor_agent", "cli_agent/devin"] {
        let p = provider(experimental);
        let (ready, hint) = provider_status(&settings, Some(&p), false);
        match hint {
            Some(AssistantProviderHint::CliAgentExperimental) => {
                assert!(ready, "{experimental}: detected → ready + advisory");
            }
            Some(AssistantProviderHint::CliAgentNotDetected) => {
                assert!(!ready, "{experimental}: absent → not detected");
            }
            other => panic!("{experimental}: unexpected hint {other:?}"),
        }
    }
    // Non-experimental adapters are unaffected.
    let codex = provider("cli_agent/codex");
    let mut enabled = AppSettings::default();
    enabled.cli_agent_configs.insert(
        "cli_agent/codex".to_string(),
        CliAgentConfig {
            enabled: true,
            ..CliAgentConfig::default()
        },
    );
    let (_, hint) = provider_status(&enabled, Some(&codex), false);
    assert_ne!(hint, Some(AssistantProviderHint::CliAgentExperimental));
}

/// The in-flight handle must be stored under the same lock acquisition
/// that flips the phase — otherwise a turn that resolves before the
/// store lands clears `in_flight` first and leaves a dead handle behind
/// (every later send then returns Busy). This reproduces `send`'s
/// critical section against an instantly-completing task.
#[tokio::test(flavor = "current_thread")]
async fn fast_finishing_turn_cannot_orphan_the_in_flight_handle() {
    let shared = std::sync::Arc::new(std::sync::Mutex::new(AssistantSession::default()));
    {
        // Mirrors send(): under one guard — flip phase, spawn a task that
        // commits an outcome as soon as it can lock, store its handle.
        let mut session = shared.lock().unwrap();
        session.generation = 1;
        session.phase = AssistantPhase::Thinking;
        let task = tauri::async_runtime::spawn({
            let shared = std::sync::Arc::clone(&shared);
            async move {
                // Stand-in for run_turn's commit: runs as soon as the
                // spawning guard releases.
                let mut s = shared.lock().unwrap();
                if s.generation == 1 {
                    s.in_flight = None;
                    s.phase = AssistantPhase::Idle;
                }
            }
        });
        session.in_flight = Some(task);
    }
    // Let the spawned task commit, then verify nothing was orphaned.
    tauri::async_runtime::spawn_blocking(|| {
        std::thread::sleep(std::time::Duration::from_millis(20))
    })
    .await
    .unwrap();
    let session = shared.lock().unwrap();
    assert!(
        session.in_flight.is_none(),
        "a completed turn must not leave a dead in_flight handle"
    );
    assert_eq!(session.phase, AssistantPhase::Idle);
}

#[test]
fn cap_response_marks_truncation() {
    let long = "x".repeat(MAX_RESPONSE_CHARS + 10);
    let capped = cap_response(long);
    assert!(capped.ends_with(TRUNCATED_MARKER));
    assert!(capped.chars().count() <= MAX_RESPONSE_CHARS + TRUNCATED_MARKER.len() + 2);
}

#[test]
fn provider_status_explains_each_unready_cause() {
    let mut settings = AppSettings::default();
    settings
        .post_process_models
        .insert("openai".to_string(), "gpt".to_string());
    let openai = provider("openai");

    // No provider at all.
    assert_eq!(
        provider_status(&settings, None, false),
        (false, Some(AssistantProviderHint::NoProvider))
    );
    // BYOK without key / with key.
    assert_eq!(
        provider_status(&settings, Some(&openai), false),
        (false, Some(AssistantProviderHint::MissingApiKey))
    );
    assert_eq!(
        provider_status(&settings, Some(&openai), true),
        (true, None)
    );
    // Offline gates even a fully configured provider.
    settings.offline_mode = true;
    assert_eq!(
        provider_status(&settings, Some(&openai), true),
        (false, Some(AssistantProviderHint::Offline))
    );
    settings.offline_mode = false;

    // Missing model.
    settings
        .post_process_models
        .insert("openai".into(), String::new());
    assert_eq!(
        provider_status(&settings, Some(&openai), true),
        (false, Some(AssistantProviderHint::MissingModel))
    );
}

#[test]
fn cli_agent_status_gates_on_enabled_and_detection() {
    let settings = AppSettings::default();
    let codex = provider("cli_agent/codex");
    // Machine-dependent detection — pin the disabled arm and the
    // enabled-but-undetected arm only when no codex is on PATH.
    let mut disabled = settings.clone();
    disabled.cli_agent_configs.insert(
        "cli_agent/codex".to_string(),
        CliAgentConfig {
            enabled: false,
            ..CliAgentConfig::default()
        },
    );
    assert_eq!(
        provider_status(&disabled, Some(&codex), false),
        (false, Some(AssistantProviderHint::CliAgentDisabled))
    );
}

#[test]
fn choose_provider_prefers_explicit_setting() {
    let mut settings = AppSettings {
        post_process_provider_id: "openai".to_string(),
        assistant_provider_id: Some("cli_agent/claude".to_string()),
        ..Default::default()
    };
    settings
        .post_process_models
        .insert("openai".to_string(), "gpt".to_string());
    let chosen = choose_provider(&settings, |_| true, |_| true);
    assert_eq!(chosen.map(|p| p.id.as_str()), Some("cli_agent/claude"));
}

#[test]
fn choose_provider_auto_prefers_usable_byok_then_detected_cli() {
    let mut settings = AppSettings {
        post_process_provider_id: "openai".to_string(),
        ..Default::default()
    };
    settings
        .post_process_models
        .insert("openai".to_string(), "gpt".to_string());
    // Usable BYOK wins over a detected CLI agent.
    let chosen = choose_provider(&settings, |_| true, |_| true);
    assert_eq!(chosen.map(|p| p.id.as_str()), Some("openai"));
    // No key → the detected CLI agent takes over.
    let chosen = choose_provider(&settings, |_| false, |_| true);
    assert_eq!(chosen.map(|p| p.id.as_str()), Some("cli_agent/codex"));
    // Nothing usable → BYOK still surfaces so the hint explains the fix.
    let chosen = choose_provider(&settings, |_| false, |_| false);
    assert_eq!(chosen.map(|p| p.id.as_str()), Some("openai"));
}

#[test]
fn panel_origin_centers_and_docks_above_the_work_area() {
    // 1920x1080 work area at 1x scale — physical px out.
    let (x, y) = panel_origin((0, 0, 1920, 1080), 1.0);
    assert_eq!(x, (1920.0 - PANEL_WIDTH) / 2.0);
    assert_eq!(y, 1080.0 - PANEL_HEIGHT - PANEL_BOTTOM_MARGIN);
    // At 2x scale the same logical rect lands on doubled physical coords.
    let (x2, y2) = panel_origin((0, 0, 3840, 2160), 2.0);
    assert_eq!(x2, (3840.0 - PANEL_WIDTH * 2.0) / 2.0);
    assert_eq!(y2, 2160.0 - (PANEL_HEIGHT + PANEL_BOTTOM_MARGIN) * 2.0);
    // Short work areas clamp at the top edge.
    let (_x, y3) = panel_origin((0, 0, 800, 400), 1.0);
    assert_eq!(y3, 0.0);
}

#[test]
fn panel_origin_stays_on_a_negative_coordinate_monitor() {
    // Secondary monitor left of the primary: work area x starts at
    // -1920. Clamping to 0 would teleport the panel to the primary.
    let (x, y) = panel_origin((-1920, -60, 1920, 1080), 1.0);
    assert_eq!(x, -1920.0 + (1920.0 - PANEL_WIDTH) / 2.0);
    assert_eq!(y, -60.0 + 1080.0 - PANEL_HEIGHT - PANEL_BOTTOM_MARGIN);
    // Even a degenerately small negative-origin area clamps to *its*
    // origin, never to global zero.
    let (x4, y4) = panel_origin((-800, -600, 400, 300), 1.0);
    assert_eq!(x4, -800.0);
    assert_eq!(y4, -600.0);
}

// ── T-092: drag/pin placement (FR-012-16, AC-012-04) ──────────────────

const PANEL: (f64, f64) = (PANEL_WIDTH, PANEL_HEIGHT);

fn monitor(name: Option<&str>, area: (i32, i32, i32, i32), content_scale: f64) -> PanelMonitor {
    PanelMonitor {
        name: name.map(str::to_string),
        work_area: area,
        content_scale,
    }
}

fn saved_pos(
    x: i32,
    y: i32,
    rel_x: f64,
    rel_y: f64,
    monitor_name: Option<&str>,
) -> AssistantPanelPosition {
    AssistantPanelPosition {
        x,
        y,
        rel_x,
        rel_y,
        monitor_name: monitor_name.map(str::to_string),
    }
}

#[test]
fn clamp_origin_pulls_the_rect_inside_the_area() {
    // Fully inside stays put.
    assert_eq!(
        clamp_origin_to_area(100, 50, 420, 560, (0, 0, 1920, 1080)),
        (100, 50)
    );
    // Hanging off the right/bottom edge gets pulled back.
    assert_eq!(
        clamp_origin_to_area(1700, 900, 420, 560, (0, 0, 1920, 1080)),
        (1500, 520)
    );
    // Negative origins clamp to the area's origin.
    assert_eq!(
        clamp_origin_to_area(-40, -10, 420, 560, (0, 0, 1920, 1080)),
        (0, 0)
    );
    // A rect larger than the area pins to its origin.
    assert_eq!(
        clamp_origin_to_area(300, 300, 4000, 2000, (0, 0, 1920, 1080)),
        (0, 0)
    );
    // Negative-coordinate monitor (secondary left of the primary).
    assert_eq!(
        clamp_origin_to_area(-2600, 400, 420, 560, (-2560, -200, 2560, 1440)),
        (-2560, 400)
    );
}

#[test]
fn resolve_position_returns_none_without_saved_position_or_monitors() {
    let monitors = vec![monitor(Some("A"), (0, 0, 1920, 1080), 1.0)];
    assert_eq!(resolve_panel_position(None, &monitors, 0, PANEL), None);
    let saved = saved_pos(100, 100, 0.5, 0.5, Some("A"));
    assert_eq!(resolve_panel_position(Some(&saved), &[], 0, PANEL), None);
}

#[test]
fn resolve_position_restores_the_saved_spot_when_the_monitor_is_unchanged() {
    let monitors = vec![monitor(Some("A"), (0, 0, 1920, 1040), 1.0)];
    let saved = saved_pos(1400, 350, 1400.0 / 1920.0, 350.0 / 1040.0, Some("A"));
    assert_eq!(
        resolve_panel_position(Some(&saved), &monitors, 0, PANEL),
        Some((1400, 350, 0))
    );
}

#[test]
fn resolve_position_clamps_a_partially_offscreen_rect_back_inside() {
    let monitors = vec![monitor(Some("A"), (0, 0, 1920, 1040), 1.0)];
    // The saved point fell off every work area (e.g. the taskbar grew);
    // the named monitor still exists, so it is clamped back inside —
    // FR-012-16 "respeita bordas".
    let saved = saved_pos(1900, 200, 1900.0 / 1920.0, 200.0 / 1040.0, Some("A"));
    assert_eq!(
        resolve_panel_position(Some(&saved), &monitors, 0, PANEL),
        Some((1480, 200, 0))
    );
}

#[test]
fn resolve_position_uses_the_named_monitor_when_its_bounds_moved() {
    // Monitor "B" kept its name but now spans 1920..4480; the saved
    // point (4500) falls off every work area → clamp into B anyway.
    let monitors = vec![
        monitor(Some("A"), (0, 0, 1920, 1040), 1.0),
        monitor(Some("B"), (1920, 0, 2560, 1440), 1.0),
    ];
    let saved = saved_pos(4500, 300, 0.9, 0.2, Some("B"));
    assert_eq!(
        resolve_panel_position(Some(&saved), &monitors, 0, PANEL),
        Some((4040, 300, 1))
    );
}

#[test]
fn resolve_position_falls_back_to_primary_at_the_relative_spot() {
    // The saved monitor is gone entirely (laptop undocked) — AC-012-04:
    // land on the primary at the same work-area fraction, clamped.
    let monitors = vec![monitor(Some("PRIMARY"), (0, 0, 1920, 1040), 1.0)];
    let saved = saved_pos(6000, 800, 0.9, 0.6, Some("GONE"));
    // x = 0.9 * 1920 = 1728 → clamped to 1920 - 440 = 1480.
    // y = 0.6 * 1040 = 624 → 624 + 560 overflows → 1040 - 560 = 480.
    assert_eq!(
        resolve_panel_position(Some(&saved), &monitors, 0, PANEL),
        Some((1480, 400, 0))
    );
}

#[test]
fn resolve_position_scales_the_panel_by_the_target_monitor() {
    // 2x monitor: the 440x640 CSS footprint is 880x1280 physical —
    // containment and clamping use the scaled size.
    let monitors = vec![monitor(Some("A"), (0, 0, 3840, 2080), 2.0)];
    let saved = saved_pos(3600, 1800, 0.9, 0.9, Some("A"));
    assert_eq!(
        resolve_panel_position(Some(&saved), &monitors, 0, PANEL),
        Some((3840 - 880, 2080 - 1280, 0))
    );
}

#[test]
fn panel_position_and_pin_roundtrip_through_settings() {
    // Fresh defaults: nothing saved, not pinned.
    let settings = AppSettings::default();
    assert_eq!(settings.assistant_panel_position, None);
    assert!(!settings.assistant_panel_pinned);

    // A partial stored object still deserializes — per-field defaults
    // keep a hand-edited or forward-written store loadable.
    let value = serde_json::json!({
        "assistant_panel_position": { "x": 100, "y": 200 },
        "assistant_panel_pinned": true
    });
    let settings: AppSettings = serde_json::from_value(value)
        .unwrap_or_else(|e| panic!("partial assistant settings must load: {e}"));
    assert!(settings.assistant_panel_pinned);
    let Some(pos) = settings.assistant_panel_position else {
        panic!("position must survive the roundtrip");
    };
    assert_eq!((pos.x, pos.y), (100, 200));
    assert_eq!(pos.monitor_name, None);
}

#[test]
fn panel_size_fits_small_and_high_dpi_work_areas() {
    assert_eq!(panel_size((0, 0, 1920, 1080), 1.0), (440, 640));
    assert_eq!(panel_size((-800, -400, 350, 300), 2.0), (350, 300));
    assert_eq!(panel_size((0, 0, 1000, 700), 1.5), (660, 700));
    assert_eq!(panel_size((0, 0, 0, 0), 1.0), (1, 1));
}

#[test]
fn new_conversation_clears_capture_and_invalidates_trailing_transcript() {
    let mut session = AssistantSession {
        open: true,
        dictation_routed: true,
        dictating: true,
        messages: vec![msg("user", "Old conversation")],
        pending_prompt: Some("Queued".into()),
        phase: AssistantPhase::Error,
        error_detail: Some("Previous failure".into()),
        ..AssistantSession::default()
    };
    let captured_epoch = session.dictation_epoch;
    reset_conversation(&mut session);
    assert!(session.open);
    assert!(!session.dictating);
    assert!(!session.dictation_routed);
    assert!(session.messages.is_empty());
    assert!(session.pending_prompt.is_none());
    assert!(session.error_detail.is_none());
    assert_eq!(session.phase, AssistantPhase::Idle);
    assert!(!accepts_dictation(&session, captured_epoch));
    assert!(accepts_dictation(&session, session.dictation_epoch));
    session.open = false;
    assert!(!accepts_dictation(&session, session.dictation_epoch));
}

#[test]
fn dictation_button_stops_capture_even_if_provider_becomes_unavailable() {
    assert!(can_toggle_dictation(true, false, Some("assistant")));
    assert!(can_toggle_dictation(false, true, None));
    assert!(!can_toggle_dictation(false, false, None));
    assert!(!can_toggle_dictation(false, true, Some("transcribe")));
    assert!(!can_toggle_dictation(false, true, Some("assistant")));
}

#[test]
fn persisted_panel_on_small_monitor_uses_the_capped_footprint() {
    let monitors = vec![monitor(Some("SMALL"), (-350, -300, 350, 300), 2.0)];
    let saved = saved_pos(-100, -100, 0.5, 0.5, Some("SMALL"));
    assert_eq!(
        resolve_panel_position(Some(&saved), &monitors, 0, PANEL),
        Some((-350, -300, 0))
    );
}

#[test]
fn closing_invalidates_a_reserved_start_and_duplicate_inputs_do_not_toggle_it() {
    let mut session = AssistantSession {
        open: true,
        ..AssistantSession::default()
    };
    let epoch = reserve_assistant_input(&mut session).expect("open assistant accepts start");
    assert!(session.dictation_pending);
    assert!(reserve_assistant_input(&mut session).is_none());
    reset_conversation(&mut session);
    assert!(!session.dictation_pending);
    assert!(!input_epoch_current(&session, epoch));
    let new_epoch = reserve_assistant_input(&mut session).expect("new start");
    assert_ne!(epoch, new_epoch);
    assert!(!input_epoch_current(&session, epoch));
    assert!(input_epoch_current(&session, new_epoch));
    session.closing = true;
    assert!(!input_epoch_current(&session, new_epoch));
    assert!(reserve_assistant_input(&mut session).is_none());
    session.closing = false;
    session.dictation_pending = false;
    assert_eq!(reserve_assistant_input(&mut session), Some(new_epoch));
    session.open = false;
    assert!(!input_epoch_current(&session, new_epoch));
    assert!(reserve_assistant_input(&mut session).is_none());
}

#[test]
fn explicit_windows_transparency_opt_out_disables_native_acrylic() {
    assert!(!super::native::transparency_allowed(Some(0)));
    assert!(super::native::transparency_allowed(Some(1)));
    assert!(super::native::transparency_allowed(None));
}

#[test]
fn stale_start_and_cleanup_cannot_consume_a_newer_reservation() {
    let mut session = AssistantSession {
        open: true,
        ..AssistantSession::default()
    };
    let old = reserve_assistant_input(&mut session).expect("first reservation");
    reset_conversation(&mut session);
    let new = reserve_assistant_input(&mut session).expect("second reservation");
    assert!(!reserved_start_current(&session, Some(old)));
    assert!(!reserved_start_current(&session, None));
    assert!(reserved_start_current(&session, Some(new)));
    assert!(!cancel_reserved_epoch(&mut session, old));
    assert!(session.dictation_pending);
    assert!(cancel_reserved_epoch(&mut session, new));
    assert!(!session.dictation_pending);
}

#[test]
fn an_old_stop_drops_its_transcript_without_consuming_the_new_capture() {
    let mut session = AssistantSession {
        open: true,
        ..AssistantSession::default()
    };
    let old = reserve_assistant_input(&mut session).expect("old");
    reset_conversation(&mut session);
    let new = reserve_assistant_input(&mut session).expect("new");
    assert_eq!(
        consume_dictation_route(&mut session, "assistant", Some(old)),
        (DictationRoute::Drop, old)
    );
    assert!(session.dictation_pending);
    assert_eq!(
        consume_dictation_route(&mut session, "assistant", None),
        (DictationRoute::Drop, 0)
    );
    assert_eq!(
        consume_dictation_route(&mut session, "assistant", Some(new)),
        (DictationRoute::Send, new)
    );
    assert!(!session.dictation_routed);
    assert_eq!(
        consume_dictation_route(&mut session, "assistant", Some(new)),
        (DictationRoute::Drop, new)
    );
}

#[test]
fn ordinary_dictation_never_consumes_an_assistant_reservation() {
    let mut session = AssistantSession {
        open: true,
        ..AssistantSession::default()
    };
    let epoch = reserve_assistant_input(&mut session).expect("reserved");
    assert_eq!(
        consume_dictation_route(&mut session, "transcribe", Some(epoch)),
        (DictationRoute::Normal, 0)
    );
    assert!(session.dictation_pending);
    assert!(input_epoch_current(&session, epoch));
}

#[test]
fn debounced_coordinator_input_can_release_its_reservation() {
    use crate::settings::ShortcutActivation;
    use crate::transcription_coordinator::{CoordinatorState, InputEvent, SessionPolicy};
    use std::time::{Duration, Instant};
    let input = || InputEvent {
        binding_id: "assistant".into(),
        hotkey_string: "test".into(),
        is_pressed: true,
        mode: ShortcutActivation::Toggle,
        hold_threshold: Duration::ZERO,
        external: false,
        policy: SessionPolicy::default(),
    };
    let now = Instant::now();
    let mut machine = CoordinatorState::new();
    assert!(machine.on_input(input(), now).is_some());
    machine.on_start_result("assistant", false, now);
    let mut session = AssistantSession {
        open: true,
        ..AssistantSession::default()
    };
    let epoch = reserve_assistant_input(&mut session).expect("reserve");
    assert!(machine
        .on_input(input(), now + Duration::from_millis(1))
        .is_none());
    assert!(cancel_reserved_epoch(&mut session, epoch));
    assert!(!session.dictation_pending);
    assert!(reserve_assistant_input(&mut session).is_some());
}

#[test]
fn terminal_feedback_does_not_block_a_fresh_assistant_capture() {
    use crate::settings::ShortcutActivation;
    use crate::transcription_coordinator::{
        CoordinatorState, InputEvent, PipelineOutcome, SessionPolicy,
    };
    use std::time::{Duration, Instant};
    let now = Instant::now();
    let input = || InputEvent {
        binding_id: "assistant".into(),
        hotkey_string: "test".into(),
        is_pressed: true,
        mode: ShortcutActivation::Toggle,
        hold_threshold: Duration::ZERO,
        external: true,
        policy: SessionPolicy::default(),
    };
    let mut machine = CoordinatorState::new();
    assert!(machine.accepts_assistant_start());
    machine.on_input(input(), now);
    assert!(!machine.accepts_assistant_start());
    machine.on_input(input(), now + Duration::from_millis(50));
    assert!(!machine.accepts_assistant_start());
    machine.on_pipeline_finished(PipelineOutcome::Done, now + Duration::from_millis(60));
    assert!(machine.current_session_snapshot().is_some());
    assert!(machine.accepts_assistant_start());
    machine.on_input(input(), now + Duration::from_millis(100));
    machine.on_start_result("assistant", false, now + Duration::from_millis(101));
    assert!(machine.accepts_assistant_start());
}
