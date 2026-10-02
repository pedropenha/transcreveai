//! Meeting session manager (T-064; FR-009-01/02, FR-009-06..09, AC-009-08).
//!
//! A single worker thread serializes every session input — the `meeting_*`
//! commands, `detector://start-requested` / `notetaker://start-requested`
//! cross-lane events and `MeetingCaptureEvent`s — through the pure
//! [`SessionMachine`] (`machine` module) and executes its [`Effect`]s against
//! SQLite, the capture plumbing and IPC. Same shape as
//! `transcription_coordinator`: the machine owns policy and timing; the
//! shell in `worker` owns `AppHandle`, the db connection and the live
//! [`MeetingCapture`].
//!
//! Post-processing is T-067's: when a meeting enters `processing` the worker
//! emits `meeting://process-requested` (`{ meeting_id }`) and stops there.

mod machine;
mod worker;

pub use machine::{
    clamp_meeting_max_minutes, MeetingPolicy, MeetingStateEvent, SessionMachine,
    DETECTOR_START_REQUESTED_EVENT, MEETING_PROCESS_REQUESTED_EVENT, MEETING_STATE_EVENT,
    TOAST_SHOW_EVENT,
};
pub(crate) use machine::{Effect, StopReason, ToastKind};

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use serde::Serialize;
use tauri::{AppHandle, Listener};

use crate::commands::{CommandError, CommandErrorCode, CommandResult};
use crate::db::meetings::Meeting;
use crate::meeting::blocks::{
    meeting_audio_dir, meetings_root, scan_meeting_blocks, SealedBlock, Track,
};

/// Whether a meeting session is active (recording or paused). Read by the
/// tray (`meeting_active` flag + red icon, FR-009-07) and by
/// `tray::quit_needs_confirmation` (FR-010-15) without going through the
/// worker thread.
pub(crate) static MEETING_ACTIVE: AtomicBool = AtomicBool::new(false);

pub fn meeting_recording_active() -> bool {
    MEETING_ACTIVE.load(Ordering::SeqCst)
}

/// What decides whether a sealed block carries speech (FR-009-09). Behind a
/// trait so tests inject silence/speech without depending on WAV content.
pub trait SpeechSignal: Send {
    fn has_speech(&self, block: &SealedBlock) -> bool;
}

/// Production signal: RMS of the sealed WAV block above a small threshold.
/// Reading a 60 s block (~1.9 MB) once per minute per track is well inside
/// the NFR-009-01 CPU budget.
pub struct RmsSpeechSignal {
    /// Minimum normalized RMS that counts as speech (0.01 ≈ −40 dBFS).
    threshold: f32,
}

impl Default for RmsSpeechSignal {
    fn default() -> Self {
        Self { threshold: 0.01 }
    }
}

impl SpeechSignal for RmsSpeechSignal {
    fn has_speech(&self, block: &SealedBlock) -> bool {
        let Ok(samples) = crate::audio_toolkit::read_wav_samples(&block.path) else {
            return false;
        };
        if samples.is_empty() {
            return false;
        }
        let mean_square = samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32;
        mean_square.sqrt() >= self.threshold
    }
}

/// Who asked for a meeting and how it should capture (FR-009-01).
#[derive(Clone, Debug)]
pub struct StartRequest {
    /// 'auto_prompt' | 'auto_start' | 'manual' | 'in_person'.
    pub detection: String,
    pub app_label: Option<String>,
    pub app_exe: Option<String>,
    /// Full path of the detected executable — persisted (after
    /// `app_icon::sanitize_exe_path`) so the Notetaker list can show its icon.
    pub app_exe_path: Option<String>,
    /// `true` = mic only ("Presencial" / detector `mic_only`) — the system
    /// track stays off.
    pub mic_only: bool,
    /// The `detector://meeting` id this start answers (FR-008-14 linkage):
    /// a `meeting_ended` event for it auto-stops the session. `None` for
    /// manual starts — they are never auto-stopped.
    pub detection_id: Option<String>,
}

impl StartRequest {
    /// Flow Bar ◉ / tray / Hub "Nova reunião" — manual call mode (mic +
    /// system).
    pub fn manual() -> Self {
        Self {
            detection: "manual".to_string(),
            app_label: None,
            app_exe: None,
            app_exe_path: None,
            mic_only: false,
            detection_id: None,
        }
    }
}

/// `toast://show` payload (contracts.md §5).
#[derive(Clone, Debug, Serialize)]
pub struct ToastPayload {
    pub kind: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    /// Meeting the action acts on — `meeting_retry_processing` needs the id;
    /// session-scoped actions (check-in, auto-stop) ignore it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meeting_id: Option<String>,
}

/// `meeting://process-requested` payload — the T-067 seam. `Deserialize` so
/// the post-processor's event listener can parse it back.
#[derive(Clone, Debug, Serialize, serde::Deserialize)]
pub struct ProcessRequestedPayload {
    pub meeting_id: String,
}

pub(crate) enum Command {
    Start {
        req: StartRequest,
        reply: Sender<CommandResult<Meeting>>,
    },
    Pause {
        reply: Sender<CommandResult<()>>,
    },
    Resume {
        reply: Sender<CommandResult<()>>,
    },
    Stop {
        reply: Sender<CommandResult<()>>,
    },
    Extend {
        reply: Sender<CommandResult<()>>,
    },
    Checkin {
        keep_recording: bool,
        reply: Sender<CommandResult<()>>,
    },
    /// FR-009-10 (T-065): a `session://state` transition — `capturing` is
    /// `true` while the dictation session owns the mic (`arming`/`recording`).
    /// Routed through the command channel so the worker's meeting clock stays
    /// the single source of timestamp truth.
    Dictation {
        capturing: bool,
    },
    /// `detector://meeting` reported `meeting_ended` for this id — the
    /// machine decides whether it owns the link (FR-008-14, T-069).
    DetectionEnded {
        detection_id: String,
    },
    /// "Continuar gravando" on the auto-stop toast — cancels the pending
    /// 15 s stop (FR-008-14).
    ContinueRecording {
        reply: Sender<CommandResult<()>>,
    },
}

/// Handle to the session worker. Cheap to clone (channel + shared snapshot);
/// Tauri manages one instance.
pub struct MeetingSessionManager {
    tx: Sender<Command>,
    /// Last emitted `meeting://state` — `meeting_current` answers
    /// late-mounted frontends (Flow Bar, meeting window) without a round
    /// trip through the worker.
    snapshot: Arc<Mutex<Option<MeetingStateEvent>>>,
}

fn meeting_state_terminal(status: &str) -> bool {
    matches!(status, "ready" | "error" | "recovered")
}

/// Whether an event may replace the snapshot a late-mounted Flow Bar reads.
/// This ordering guard is deliberately separate from broadcasting: a stale
/// event must not cover the snapshot, but it is still emitted so Hub rows and
/// already-mounted meeting windows can update their own meeting id.
fn meeting_snapshot_accepts_state(
    current: Option<&MeetingStateEvent>,
    incoming: &MeetingStateEvent,
) -> bool {
    let Some(current) = current else {
        return true;
    };

    if current.meeting_id == incoming.meeting_id {
        // A retry legitimately moves ready/error/recovered back to
        // processing. Live ticks after processing starts or after a terminal
        // state are stale capture events, not a new lifecycle.
        return incoming.status == "processing"
            || meeting_state_terminal(&incoming.status)
            || !matches!(
                current.status.as_str(),
                "processing" | "ready" | "error" | "recovered"
            );
    }

    if matches!(incoming.status.as_str(), "recording" | "paused") {
        return true;
    }

    // While another meeting is live, post-processing from an older meeting is
    // broadcast for its Hub row but must not hide the active pill. Once the
    // snapshot is terminal, only a cross-meeting `processing` may replace it —
    // a `ready`/`error`/`recovered` for a meeting this snapshot never saw
    // process is a stale replay (startup recovery, a retried old meeting), not
    // the live lifecycle. A legitimately newer meeting arrives as `processing`
    // first; its own terminal event then lands via the same-id rule above.
    meeting_state_terminal(&current.status) && incoming.status == "processing"
}

/// Accept-or-reject + commit for the late-mount snapshot. Terminal commits arm
/// the overlay dwell *before* the snapshot flips, closing the window where a
/// racing `hide_recording_overlay` could read `(terminal, dwell = 0)`.
fn commit_meeting_snapshot(
    snapshot: &Mutex<Option<MeetingStateEvent>>,
    event: &MeetingStateEvent,
) -> bool {
    let mut guard = snapshot.lock().unwrap_or_else(|e| e.into_inner());
    if !meeting_snapshot_accepts_state(guard.as_ref(), event) {
        return false;
    }
    if meeting_state_terminal(&event.status) {
        crate::overlay::arm_meeting_terminal_deadline();
    }
    *guard = Some(event.clone());
    true
}

impl Clone for MeetingSessionManager {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            snapshot: Arc::clone(&self.snapshot),
        }
    }
}

impl MeetingSessionManager {
    pub fn new(app: AppHandle) -> Self {
        let (tx, rx) = mpsc::channel();
        let snapshot = Arc::new(Mutex::new(None));
        let worker_snapshot = Arc::clone(&snapshot);

        // FR-009-10 (T-065): dictation sessions exclude mic speech and drop a
        // `dictation_marker` (AC-009-03). The capturing signal is the
        // `session://state` payload's `state` field forwarded as a command —
        // dictation owns no meeting state, the worker owns the clock.
        let dictation_tx = tx.clone();
        app.listen(
            crate::transcription_coordinator::SESSION_STATE_EVENT,
            move |event| {
                #[derive(serde::Deserialize)]
                struct SessionStateProbe {
                    state: String,
                }
                let Ok(probe) = serde_json::from_str::<SessionStateProbe>(event.payload()) else {
                    return;
                };
                let _ = dictation_tx.send(Command::Dictation {
                    capturing: crate::meeting::live::dictation_capturing(&probe.state),
                });
            },
        );

        let spawned = std::thread::Builder::new()
            .name("meeting-session".to_string())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    worker::Worker::new(app, worker_snapshot).run(rx);
                }));
                if let Err(e) = result {
                    log::error!("Meeting session worker panicked: {e:?}");
                    MEETING_ACTIVE.store(false, Ordering::SeqCst);
                }
            });
        if let Err(e) = spawned {
            log::error!("Failed to spawn meeting session worker: {e}");
        }
        Self { tx, snapshot }
    }

    /// FR-009-01/02: start the single allowed meeting. Fails with
    /// `consent_required` until `meeting_consent_accept` has run and `busy`
    /// while another meeting is active.
    pub fn request_start(&self, req: StartRequest) -> CommandResult<Meeting> {
        self.call(|reply| Command::Start { req, reply })?
    }

    pub fn request_stop(&self) -> CommandResult<()> {
        self.call(|reply| Command::Stop { reply })?
    }

    pub fn request_pause(&self) -> CommandResult<()> {
        self.call(|reply| Command::Pause { reply })?
    }

    pub fn request_resume(&self) -> CommandResult<()> {
        self.call(|reply| Command::Resume { reply })?
    }

    /// FR-009-08: "Estender 30 min" from the limit toast.
    pub fn request_extend(&self) -> CommandResult<()> {
        self.call(|reply| Command::Extend { reply })?
    }

    /// FR-009-09 check-in answer; `keep_recording = false` stops the meeting.
    pub fn request_checkin_respond(&self, keep_recording: bool) -> CommandResult<()> {
        self.call(|reply| Command::Checkin {
            keep_recording,
            reply,
        })?
    }

    /// FR-008-14 (T-069): a `detector://meeting {ended, meeting_ended: true}`
    /// for the session's linked detection arms the 15 s auto-stop toast.
    /// Fire-and-forget — an idle session is a no-op by design.
    pub fn detection_ended(&self, detection_id: String) {
        if let Err(e) = self.tx.send(Command::DetectionEnded { detection_id }) {
            log::warn!("Meeting session unavailable for detection end: {e}");
        }
    }

    /// FR-008-14: "Continuar gravando" on the auto-stop toast.
    pub fn request_continue_recording(&self) -> CommandResult<()> {
        self.call(|reply| Command::ContinueRecording { reply })?
    }

    /// A meeting is recording or paused (the worker owns the truth).
    pub fn is_active(&self) -> bool {
        meeting_recording_active()
    }

    /// Latest `meeting://state` snapshot (`meeting_current`).
    pub fn current(&self) -> Option<MeetingStateEvent> {
        self.snapshot
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Record a lifecycle event emitted outside the session worker (the
    /// post-processor owns `processing` → `ready`/`error`). Snapshot ordering
    /// is only for late-mounted UI; it never suppresses the event broadcast.
    /// Returns whether the event may claim the Flow Bar's single meeting face.
    pub fn observe_meeting_state(&self, event: &MeetingStateEvent) -> bool {
        commit_meeting_snapshot(&self.snapshot, event)
    }

    fn call<T>(&self, mk: impl FnOnce(Sender<T>) -> Command) -> CommandResult<T> {
        let (reply, rx) = mpsc::channel();
        self.tx.send(mk(reply)).map_err(|_| {
            CommandError::logged(
                CommandErrorCode::Internal,
                "Meeting session is unavailable",
                "worker channel closed",
            )
        })?;
        rx.recv().map_err(|_| {
            CommandError::logged(
                CommandErrorCode::Internal,
                "Meeting session did not answer",
                "reply channel closed",
            )
        })
    }
}

/// The default meeting title (FR-009-12): "<App> · <dd/mm hh:mm>", or
/// "Reunião · …" when no app triggered the start.
pub(crate) fn default_title(
    app_label: Option<&str>,
    now: chrono::DateTime<chrono::Local>,
) -> String {
    let stamp = now.format("%d/%m %H:%M");
    match app_label {
        Some(label) if !label.trim().is_empty() => format!("{label} · {stamp}"),
        _ => format!("Reunião · {stamp}"),
    }
}

/// First block index per track when (re)starting a capture into `dir` —
/// `scan_meeting_blocks` is the source of truth so a resumed meeting keeps
/// numbering (and offsets, via the shared `time_base`) across pauses.
pub(crate) fn next_block_indices(dir: &Path) -> (u32, u32) {
    let scanned = scan_meeting_blocks(dir).unwrap_or_default();
    let next = |track: Track| {
        scanned
            .blocks
            .iter()
            .filter(|b| b.track == track)
            .map(|b| b.index + 1)
            .max()
            .unwrap_or(1)
    };
    (next(Track::Mic), next(Track::System))
}

/// `true` when `dir` is inside the meetings audio root — the guard
/// `meeting_delete` applies before `remove_dir_all` so a tampered
/// `audio_dir` can never reach outside `app_data`.
fn dir_inside_meetings_root(app_data_dir: &Path, dir: &Path) -> bool {
    dir.starts_with(meetings_root(app_data_dir))
}

/// Localized toast text for each [`ToastKind`]. `lang` is the normalized
/// `app_language` (`"pt-BR"` or `"en"`).
pub(crate) fn toast_message(kind: ToastKind, lang: &str) -> String {
    let pt = lang == "pt-BR";
    let text = match kind {
        ToastKind::MicUnavailable => {
            if pt {
                "Microfone indisponível — outro app pode estar usando o microfone."
            } else {
                "Microphone unavailable — another app may be using it."
            }
        }
        ToastKind::SystemUnavailable => {
            if pt {
                "Os outros participantes não estão sendo capturados."
            } else {
                "The other participants are not being captured."
            }
        }
        ToastKind::LimitWarning => {
            if pt {
                "A reunião atingirá o limite de duração em 5 minutos."
            } else {
                "The meeting will reach the duration limit in 5 minutes."
            }
        }
        ToastKind::SilenceCheckin => {
            if pt {
                "Ainda em reunião?"
            } else {
                "Still in the meeting?"
            }
        }
        ToastKind::WriteFailed => {
            if pt {
                "Falha ao gravar o áudio da reunião — o disco pode estar cheio."
            } else {
                "Failed to write meeting audio — the disk may be full."
            }
        }
        ToastKind::AutoStop => {
            if pt {
                "A reunião terminou — finalizando em 15 s"
            } else {
                "The meeting ended — finishing in 15 s"
            }
        }
        ToastKind::ConsentReminder => {
            if pt {
                "Informe aos participantes que a reunião está sendo transcrita."
            } else {
                "Let participants know the meeting is being transcribed."
            }
        }
    };
    text.to_string()
}

/// Open the app db for session use (same path resolution as recovery).
pub(crate) fn open_session_db(app: &AppHandle) -> CommandResult<Connection> {
    let dir = crate::portable::app_data_dir(app).map_err(|e| {
        CommandError::logged(CommandErrorCode::Internal, "Failed to resolve app data", e)
    })?;
    let path = crate::db::database_path(&dir).map_err(|e| {
        CommandError::logged(CommandErrorCode::Internal, "Failed to resolve database", e)
    })?;
    crate::db::open_connection(&path)
        .map_err(|e| CommandError::logged(CommandErrorCode::Internal, "Failed to open database", e))
}

/// Delete a meeting's audio dir — guarded to `audio/meetings/` so a tampered
/// row can never delete outside `app_data`.
pub(crate) fn remove_meeting_audio_dir(app_data_dir: &Path, meeting_id: &str) -> CommandResult<()> {
    // Meeting ids are UUIDs minted by `Meeting::new`. Enforcing that here
    // also kills path traversal — `"../dictations"` is not a valid UUID, so
    // a tampered `meeting_delete` argument can never escape the root before
    // the component-level guard below even runs (`Path::starts_with` does
    // not normalize `..`).
    let canonical = uuid::Uuid::parse_str(meeting_id)
        .map(|uuid| uuid.hyphenated().to_string())
        .map_err(|_| CommandError::new(CommandErrorCode::InvalidInput, "Invalid meeting id"))?;
    let dir = meeting_audio_dir(app_data_dir, &canonical);
    if !dir_inside_meetings_root(app_data_dir, &dir) {
        return Err(CommandError::new(
            CommandErrorCode::InvalidInput,
            "Refusing to delete audio outside the meetings directory",
        ));
    }
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => Ok(()),
        // Never recorded / already gone: deleting is idempotent.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(CommandError::logged(
            CommandErrorCode::Internal,
            "Failed to delete the meeting audio",
            e,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meeting::blocks::BlockWriter;
    use std::time::Instant;

    #[test]
    fn title_uses_app_label_or_generic_prefix() {
        let now = chrono::Local::now();
        let titled = default_title(Some("Zoom"), now);
        assert!(titled.starts_with("Zoom · "), "{titled}");
        assert!(default_title(None, now).starts_with("Reunião · "));
        assert!(default_title(Some("  "), now).starts_with("Reunião · "));
    }

    #[test]
    fn resume_indices_continue_on_disk_numbering() {
        let dir = tempfile::tempdir().unwrap();
        let t0 = Instant::now();
        let mut mic = BlockWriter::with_block_samples(dir.path(), Track::Mic, t0, 10).unwrap();
        let mut sys = BlockWriter::with_block_samples(dir.path(), Track::System, t0, 10).unwrap();
        mic.push(&[0.1; 25], t0).unwrap();
        sys.push(&[0.2; 10], t0).unwrap();
        mic.finish(t0).unwrap();
        sys.finish(t0).unwrap();

        assert_eq!(next_block_indices(dir.path()), (4, 2));
        assert_eq!(next_block_indices(&dir.path().join("nope")), (1, 1));
    }

    #[test]
    fn audio_dir_guard_confines_deletes_to_meetings_root() {
        let root = Path::new("data");
        assert!(dir_inside_meetings_root(
            root,
            &meeting_audio_dir(root, "abc")
        ));
        assert!(!dir_inside_meetings_root(root, Path::new("data/other")));
        assert!(!dir_inside_meetings_root(root, Path::new("elsewhere")));
        // `..` components pass a naive `starts_with` — the real defense is
        // the UUID gate in `remove_meeting_audio_dir`.
        assert!(dir_inside_meetings_root(
            root,
            &meeting_audio_dir(root, "../dictations")
        ));
    }

    #[test]
    fn remove_audio_dir_rejects_non_uuid_ids() {
        // A frontend-supplied id containing `..` would otherwise resolve
        // outside `audio/meetings/` once the OS normalizes the path.
        for bad in ["../dictations", "../../", "a/b", ".", "..", ""] {
            let err = remove_meeting_audio_dir(Path::new("data"), bad)
                .expect_err("non-UUID id must be rejected");
            assert_eq!(
                err.code,
                CommandErrorCode::InvalidInput,
                "id {bad:?} should fail validation"
            );
        }
        // A well-formed UUID for a meeting that never recorded deletes
        // nothing and succeeds (idempotent).
        remove_meeting_audio_dir(Path::new("data"), "9f0c0000-0000-4000-8000-000000000000")
            .expect("UUID id");
    }

    #[test]
    fn meeting_snapshot_accepts_terminal_state_for_the_same_meeting_only() {
        let event = |id: &str, status: &str| MeetingStateEvent {
            meeting_id: id.to_string(),
            status: status.to_string(),
            elapsed_ms: 1_000,
        };
        let recording = event("meeting-1", "recording");
        let processing = event("meeting-1", "processing");
        let ready = event("meeting-1", "ready");

        assert!(meeting_snapshot_accepts_state(
            Some(&recording),
            &processing
        ));
        assert!(meeting_snapshot_accepts_state(Some(&processing), &ready));
        // A stale post-processing event from an older meeting must not cover
        // the newer live pill; a new active session may replace a terminal one.
        assert!(!meeting_snapshot_accepts_state(
            Some(&recording),
            &event("meeting-2", "processing"),
        ));
        assert!(meeting_snapshot_accepts_state(
            Some(&ready),
            &event("meeting-2", "recording"),
        ));
        // A retry moves the same meeting back to processing; a new meeting's
        // `processing` may replace an old terminal snapshot, but its terminal
        // event cannot jump in directly — it lands via the same-id rule after
        // its own processing was observed. A replayed terminal from an unseen
        // meeting (startup recovery, stale worker) must not claim the bar.
        assert!(meeting_snapshot_accepts_state(Some(&ready), &processing));
        assert!(meeting_snapshot_accepts_state(
            Some(&ready),
            &event("meeting-2", "processing")
        ));
        assert!(!meeting_snapshot_accepts_state(
            Some(&ready),
            &event("meeting-2", "ready")
        ));
        assert!(!meeting_snapshot_accepts_state(
            Some(&ready),
            &event("meeting-2", "recovered")
        ));
        assert!(!meeting_snapshot_accepts_state(
            Some(&recording),
            &event("meeting-2", "ready")
        ));
        // A stale live tick must not regress processing or a terminal state.
        assert!(!meeting_snapshot_accepts_state(
            Some(&processing),
            &recording
        ));
        assert!(!meeting_snapshot_accepts_state(Some(&ready), &recording));
    }

    #[test]
    fn speech_signal_reads_rms_from_sealed_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let t0 = Instant::now();
        let mut loud = BlockWriter::with_block_samples(dir.path(), Track::Mic, t0, 100).unwrap();
        loud.push(&[0.4; 100], t0).unwrap();
        let loud = loud.finish(t0).unwrap();
        // Different track — both writers would otherwise seal `mic-0001.wav`.
        let mut quiet =
            BlockWriter::with_block_samples(dir.path(), Track::System, t0, 100).unwrap();
        quiet.push(&[0.0001; 100], t0).unwrap();
        let quiet = quiet.finish(t0).unwrap();

        let signal = RmsSpeechSignal::default();
        assert!(signal.has_speech(&loud[0]));
        assert!(!signal.has_speech(&quiet[0]));
    }

    #[test]
    fn toast_messages_are_localized() {
        assert_eq!(
            toast_message(ToastKind::SilenceCheckin, "pt-BR"),
            "Ainda em reunião?"
        );
        assert_eq!(
            toast_message(ToastKind::SilenceCheckin, "en"),
            "Still in the meeting?"
        );
        assert!(toast_message(ToastKind::SystemUnavailable, "pt-BR").contains("participantes"));
        // FR-009-02: the reminder is a nudge, not the clipboard payload —
        // `meeting_consent_text` is what `copy_consent` copies, not what the
        // toast prints.
        assert!(toast_message(ToastKind::ConsentReminder, "pt-BR").contains("participantes"));
        assert!(toast_message(ToastKind::ConsentReminder, "en").contains("participants"));
    }

    #[test]
    fn toast_kinds_carry_contract_action_names() {
        assert_eq!(ToastKind::LimitWarning.action(), Some("extend_30"));
        assert_eq!(ToastKind::SilenceCheckin.action(), Some("checkin"));
        assert_eq!(ToastKind::AutoStop.action(), Some("continue_recording"));
        assert_eq!(ToastKind::AutoStop.kind(), "meeting_auto_stop");
        assert_eq!(ToastKind::MicUnavailable.action(), None);
        // FR-009-02: the reminder shares the gate's `meeting_consent` kind
        // and differs only by the `copy_consent` action — the toast and
        // MeetingConsentGate split on it.
        assert_eq!(ToastKind::ConsentReminder.kind(), "meeting_consent");
        assert_eq!(ToastKind::ConsentReminder.action(), Some("copy_consent"));
    }

    #[test]
    fn auto_stop_toast_is_localized() {
        // FR-008-14 copy (spec wording), pt-BR first.
        assert!(toast_message(ToastKind::AutoStop, "pt-BR").contains("15 s"));
        assert!(toast_message(ToastKind::AutoStop, "en").contains("15 s"));
    }
}
