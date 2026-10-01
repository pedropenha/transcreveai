//! Split by concern: `session` covers the pending-release, processing and
//! session-queue behavior; `modes` covers arming, double-tap, duration
//! limits and the hold-or-toggle activation modes.

use super::*;

mod modes;
mod session;

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
