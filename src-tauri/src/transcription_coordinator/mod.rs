//! Dictation session coordinator.
//!
//! Serialises all transcription lifecycle events through a single thread to
//! eliminate race conditions between keyboard shortcuts, signals, and the
//! async transcribe-paste pipeline. The thread is a thin shell: it transports
//! commands to the pure [`CoordinatorState`] (`machine` module) and executes
//! the returned [`Effect`]s. Session transitions are journaled by the machine
//! and emitted here as `session://state` (contracts.md §5) for the Flow Bar.
//!
//! The machine implements the full FR-002-09 lifecycle
//! (`Idle → Arming → Recording → Transcribing → Processing → Inserting →
//! Done | Error`), the FIFO queue of pending sessions (FR-002-16), the
//! duration limit with its T-60 s warning (FR-002-13) and the double-tap
//! hands-free window (FR-002-07).

mod machine;

use crate::actions::ACTION_MAP;
use crate::managers::audio::AudioRecordingManager;
use crate::managers::transcription::TranscriptionManager;
use crate::settings::ShortcutActivation;
use log::{debug, error, warn};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

pub(crate) use machine::CoordinatorState;
pub(crate) use machine::{Effect, InputEvent};
pub use machine::{
    PipelineOutcome, PipelinePhase, SessionPolicy, SessionResultEvent, SessionSnapshot,
    SESSION_RESULT_EVENT, SESSION_STATE_EVENT,
};

/// Commands processed sequentially by the coordinator thread.
enum Command {
    Input(InputEvent),
    Cancel {
        recording_was_active: bool,
    },
    /// The pipeline reports an intermediate stage (Processing / Inserting).
    PipelinePhase(PipelinePhase),
    /// The pipeline finished the current session (done / failed / empty /
    /// cancelled) — drains the FIFO of pending sessions.
    PipelineFinished(PipelineOutcome),
}

pub fn is_transcribe_binding(id: &str) -> bool {
    id == "transcribe" || id == "transcribe_with_post_process"
}

pub struct TranscriptionCoordinator {
    tx: Sender<Command>,
    /// Active session readable by the pipeline (session id + capture start).
    session_snapshot: Arc<Mutex<Option<SessionSnapshot>>>,
}

impl TranscriptionCoordinator {
    pub fn new(app: AppHandle) -> Self {
        let (tx, rx) = mpsc::channel();
        let session_snapshot: Arc<Mutex<Option<SessionSnapshot>>> = Arc::new(Mutex::new(None));
        let shared = Arc::clone(&session_snapshot);

        thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut state = CoordinatorState::new();

                loop {
                    let cmd = if let Some(deadline) = state.next_deadline() {
                        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                            Ok(cmd) => cmd,
                            Err(mpsc::RecvTimeoutError::Timeout) => {
                                if let Some(effect) = state.on_deadline(Instant::now()) {
                                    run_effect(&app, &mut state, effect);
                                }
                                flush_events(&app, &shared, &mut state);
                                continue;
                            }
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        }
                    } else {
                        match rx.recv() {
                            Ok(cmd) => cmd,
                            Err(_) => break,
                        }
                    };

                    match cmd {
                        Command::Input(input) => {
                            if let Some(effect) = state.on_input(input, Instant::now()) {
                                run_effect(&app, &mut state, effect);
                            }
                        }
                        Command::Cancel {
                            recording_was_active,
                        } => state.on_cancel(recording_was_active),
                        Command::PipelinePhase(phase) => state.on_pipeline_phase(phase),
                        Command::PipelineFinished(outcome) => {
                            if let Some(effect) =
                                state.on_pipeline_finished(outcome, Instant::now())
                            {
                                run_effect(&app, &mut state, effect);
                            }
                        }
                    }
                    flush_events(&app, &shared, &mut state);
                }
                debug!("Transcription coordinator exited");
            }));
            if let Err(e) = result {
                error!("Transcription coordinator panicked: {e:?}");
            }
        });

        Self {
            tx,
            session_snapshot,
        }
    }

    /// Send a keyboard input event for a transcribe binding. `hold_threshold`
    /// only matters for [`ShortcutActivation::HoldOrToggle`]; `policy` carries
    /// the session limits (queue capacity, duration limit, double tap).
    pub fn send_input(
        &self,
        binding_id: &str,
        hotkey_string: &str,
        is_pressed: bool,
        mode: ShortcutActivation,
        hold_threshold: Duration,
        policy: SessionPolicy,
    ) {
        self.send(
            binding_id,
            hotkey_string,
            is_pressed,
            mode,
            hold_threshold,
            policy,
            false,
        );
    }

    /// Send an external trigger (SIGUSR2, CLI flag). Always a toggle press,
    /// always exempt from debounce — see [`InputEvent::external`].
    pub fn send_external_input(&self, binding_id: &str, source: &str, policy: SessionPolicy) {
        self.send(
            binding_id,
            source,
            true,
            ShortcutActivation::Toggle,
            Duration::ZERO,
            policy,
            true,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn send(
        &self,
        binding_id: &str,
        hotkey_string: &str,
        is_pressed: bool,
        mode: ShortcutActivation,
        hold_threshold: Duration,
        policy: SessionPolicy,
        external: bool,
    ) {
        if self
            .tx
            .send(Command::Input(InputEvent {
                binding_id: binding_id.to_string(),
                hotkey_string: hotkey_string.to_string(),
                is_pressed,
                mode,
                hold_threshold,
                external,
                policy,
            }))
            .is_err()
        {
            warn!("Transcription coordinator channel closed");
        }
    }

    pub fn notify_cancel(&self, recording_was_active: bool) {
        if self
            .tx
            .send(Command::Cancel {
                recording_was_active,
            })
            .is_err()
        {
            warn!("Transcription coordinator channel closed");
        }
    }

    /// The pipeline entered an intermediate stage (post-processing /
    /// insertion) for the session currently being worked.
    pub fn notify_pipeline_phase(&self, phase: PipelinePhase) {
        if self.tx.send(Command::PipelinePhase(phase)).is_err() {
            warn!("Transcription coordinator channel closed");
        }
    }

    /// The pipeline finished the current session; the coordinator records the
    /// outcome and drains the pending-session queue.
    pub fn notify_pipeline_finished(&self, outcome: PipelineOutcome) {
        if self.tx.send(Command::PipelineFinished(outcome)).is_err() {
            warn!("Transcription coordinator channel closed");
        }
    }

    /// The active session's snapshot, for the pipeline to attach the session
    /// id to `session://result` and to measure the captured audio duration.
    pub fn current_session(&self) -> Option<SessionSnapshot> {
        self.session_snapshot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

/// Emit every journaled `session://state` payload in order and keep the
/// shared session snapshot in sync with the machine.
fn flush_events(
    app: &AppHandle,
    shared: &Arc<Mutex<Option<SessionSnapshot>>>,
    state: &mut CoordinatorState,
) {
    if let Ok(mut guard) = shared.lock() {
        *guard = state.current_session_snapshot();
    }
    for event in state.take_events() {
        if let Err(e) = app.emit(SESSION_STATE_EVENT, event) {
            warn!("Failed to emit {SESSION_STATE_EVENT}: {e}");
        }
    }
}

fn run_effect(app: &AppHandle, state: &mut CoordinatorState, effect: Effect) {
    match effect {
        Effect::Start {
            binding_id,
            hotkey_string,
        } => {
            let started = start(app, &binding_id, &hotkey_string);
            state.on_start_result(&binding_id, started, Instant::now());
        }
        Effect::Stop {
            binding_id,
            hotkey_string,
        } => {
            stop(app, &binding_id, &hotkey_string);
        }
        Effect::Discard { binding_id } => {
            discard(app, &binding_id);
        }
    }
}

/// Execute a start effect; returns whether recording actually began, so the
/// state machine can roll back its optimistic transition on failure.
fn start(app: &AppHandle, binding_id: &str, hotkey_string: &str) -> bool {
    let Some(action) = ACTION_MAP.get(binding_id) else {
        warn!("No action in ACTION_MAP for '{binding_id}'");
        return false;
    };
    action.start(app, binding_id, hotkey_string);
    let recording = app
        .try_state::<Arc<AudioRecordingManager>>()
        .is_some_and(|a| a.is_recording());
    if !recording {
        debug!("Start for '{binding_id}' did not begin recording; staying idle");
    }
    recording
}

fn stop(app: &AppHandle, binding_id: &str, hotkey_string: &str) {
    let Some(action) = ACTION_MAP.get(binding_id) else {
        warn!("No action in ACTION_MAP for '{binding_id}'");
        return;
    };
    action.stop(app, binding_id, hotkey_string);
}

/// Discard an in-progress capture without transcribing (arming cancel, or a
/// tap that saw no second tap): the audio goes nowhere, the overlay hides and
/// the tray returns to idle. Separate from `cancel_current_operation`, which
/// also notifies the coordinator — this *is* the coordinator, acting.
fn discard(app: &AppHandle, binding_id: &str) {
    debug!("Discarding capture for '{binding_id}' without transcribing");
    if let Some(rm) = app.try_state::<Arc<AudioRecordingManager>>() {
        rm.cancel_recording();
    }
    if let Some(tm) = app.try_state::<Arc<TranscriptionManager>>() {
        tm.cancel_stream();
    }
    crate::shortcut::unregister_cancel_shortcut(app);
    crate::utils::hide_recording_overlay(app);
    crate::tray::set_tray_state(app, crate::tray::TrayIconState::Idle);
}
