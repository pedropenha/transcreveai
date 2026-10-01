//! The session worker — the effect shell around [`SessionMachine`].
//!
//! Runs on its own thread (`meeting-session`), serializing commands and
//! `MeetingCaptureEvent`s and turning machine [`Effect`]s into db writes,
//! capture starts/stops, tray/overlay indicator updates and `toast://show`
//! emissions. Nothing here decides policy; that lives in `machine`.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use rusqlite::Connection;
use tauri::{AppHandle, Emitter, Manager};

use super::{
    default_title, next_block_indices, open_session_db, toast_message, Command, Effect,
    MeetingPolicy, MeetingStateEvent, ProcessRequestedPayload, RmsSpeechSignal, SessionMachine,
    SpeechSignal, StartRequest, StopReason, ToastKind, ToastPayload, MEETING_ACTIVE,
    MEETING_PROCESS_REQUESTED_EVENT, MEETING_STATE_EVENT, TOAST_SHOW_EVENT,
};
use crate::audio_toolkit::audio::loopback::SystemAudioEvent;
use crate::commands::{CommandError, CommandErrorCode, CommandResult};
use crate::db::meetings::{
    Meeting, MeetingRepository, MeetingSegment, MeetingSegmentRepository, SqliteMeetingRepository,
    SqliteMeetingSegmentRepository,
};
use crate::db::notes::{Note, NoteRepository, SqliteNoteRepository};
use crate::managers::audio::AudioRecordingManager;
use crate::meeting::blocks::{meeting_audio_dir, Track};
use crate::meeting::capture::{
    CaptureConfig, MeetingCapture, MeetingCaptureEvent, MicTap, SystemSource,
};
use crate::settings::{get_settings, AppSettings};
use crate::TranscriptionCoordinator;

pub(super) struct Worker {
    app: AppHandle,
    snapshot: Arc<Mutex<Option<MeetingStateEvent>>>,
    conn: Option<Connection>,
    audio: Option<Arc<AudioRecordingManager>>,
    speech: Box<dyn SpeechSignal>,
    machine: Option<SessionMachine>,
    capture: Option<MeetingCapture>,
    capture_rx: Option<Receiver<MeetingCaptureEvent>>,
    /// The active meeting row (for status updates).
    meeting: Option<Meeting>,
    /// The meeting's monotonic base — block offsets and gap markers stay on
    /// the meeting clock across pause/resume.
    meeting_t0: Instant,
    audio_dir: Option<PathBuf>,
}

impl Worker {
    pub(super) fn new(app: AppHandle, snapshot: Arc<Mutex<Option<MeetingStateEvent>>>) -> Self {
        let audio = app
            .try_state::<Arc<AudioRecordingManager>>()
            .map(|s| s.inner().clone());
        Self {
            app,
            snapshot,
            conn: None,
            audio,
            speech: Box::new(RmsSpeechSignal::default()),
            machine: None,
            capture: None,
            capture_rx: None,
            meeting: None,
            meeting_t0: Instant::now(),
            audio_dir: None,
        }
    }

    pub(super) fn run(&mut self, rx: Receiver<Command>) {
        loop {
            // Capture events are only consumed between commands/ticks; while
            // recording the 1 s state tick bounds their latency.
            self.drain_capture_events();
            let deadline = self.machine.as_ref().and_then(|m| m.next_deadline());
            match deadline {
                Some(at) => match rx.recv_timeout(at.saturating_duration_since(Instant::now())) {
                    Ok(cmd) => self.dispatch(cmd),
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if let Some(m) = self.machine.as_mut() {
                            m.tick(Instant::now());
                        }
                        self.flush();
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                },
                None => match rx.recv() {
                    Ok(cmd) => self.dispatch(cmd),
                    Err(_) => break,
                },
            }
        }
        // Shutdown: seal whatever is still in flight so a quit mid-meeting
        // keeps the on-disk blocks valid (the recovery sweep will mark the
        // row `recovered` on next launch).
        if let Some(capture) = self.capture.take() {
            capture.stop();
        }
        MEETING_ACTIVE.store(false, Ordering::SeqCst);
        log::debug!("Meeting session worker exited");
    }

    fn dispatch(&mut self, cmd: Command) {
        match cmd {
            Command::Start { req, reply } => {
                let _ = reply.send(self.handle_start(req));
                self.flush();
            }
            Command::Pause { reply } => {
                let _ = reply.send(self.simple_machine_op(|m, now| m.pause(now)));
                self.flush();
            }
            Command::Resume { reply } => {
                let _ = reply.send(self.simple_machine_op(|m, now| m.resume(now)));
                self.flush();
            }
            Command::Stop { reply } => {
                let _ = reply.send(self.simple_machine_op(|m, now| m.stop(now, StopReason::User)));
                self.flush();
            }
            Command::Extend { reply } => {
                let _ = reply.send(self.simple_machine_op(|m, now| m.extend(now)));
                self.flush();
            }
            Command::Checkin {
                keep_recording,
                reply,
            } => {
                let result = match self.machine.as_mut() {
                    None => Err(CommandError::new(
                        CommandErrorCode::NotFound,
                        "No meeting is recording",
                    )),
                    Some(m) if !m.checkin_pending() => Err(CommandError::new(
                        CommandErrorCode::InvalidInput,
                        "No check-in is waiting for an answer",
                    )),
                    Some(m) => {
                        m.checkin_respond(keep_recording, Instant::now());
                        Ok(())
                    }
                };
                let _ = reply.send(result);
                self.flush();
            }
        }
    }

    /// Pause/resume/stop/extend share "must have an active machine".
    fn simple_machine_op(
        &mut self,
        op: impl FnOnce(&mut SessionMachine, Instant),
    ) -> CommandResult<()> {
        match self.machine.as_mut() {
            None => Err(CommandError::new(
                CommandErrorCode::NotFound,
                "No meeting is recording",
            )),
            Some(m) => {
                op(m, Instant::now());
                Ok(())
            }
        }
    }

    fn handle_start(&mut self, req: StartRequest) -> CommandResult<Meeting> {
        if self.machine.as_ref().is_some_and(|m| m.is_active()) {
            return Err(CommandError::new(
                CommandErrorCode::Busy,
                "A meeting is already being recorded",
            ));
        }
        let settings: AppSettings = get_settings(&self.app);
        // FR-009-02: first-use consent gate. The frontend turns the
        // `consent_required` code into the modal.
        if !settings.meeting_consent_acknowledged {
            return Err(CommandError::new(
                CommandErrorCode::ConsentRequired,
                "Review the meeting transcription consent notice first",
            ));
        }

        let now = Instant::now();
        let app_data = crate::portable::app_data_dir(&self.app).map_err(|e| {
            CommandError::logged(CommandErrorCode::Internal, "Failed to resolve app data", e)
        })?;

        let mut meeting = Meeting::new(
            &default_title(req.app_label.as_deref(), chrono::Local::now()),
            &req.detection,
        );
        meeting.app_label = req.app_label;
        meeting.app_exe = req.app_exe;
        meeting.capture_system_audio = !req.mic_only;
        meeting.stt_provider_id = settings.meeting_provider_id.clone();
        if settings.selected_language != "auto" {
            meeting.language = Some(settings.selected_language.clone());
        }
        let audio_dir = meeting_audio_dir(&app_data, &meeting.id);
        meeting.audio_dir = Some(audio_dir.to_string_lossy().to_string());

        // Capture first: a failed start leaves no orphaned `recording` row.
        // `start_capture` borrows `&mut self`, so no db borrow may be live.
        let (next_mic, next_system) = next_block_indices(&audio_dir);
        let capture = self.start_capture(&audio_dir, !req.mic_only, now, next_mic, next_system)?;

        let conn = self.conn()?;
        SqliteMeetingRepository::new(conn)
            .create(&meeting)
            .map_err(|e| {
                CommandError::logged(CommandErrorCode::Internal, "Failed to save the meeting", e)
            })?;
        // FR-009-13 "Minhas notas": the tab's row exists from the start;
        // T-066 edits it in place.
        let mut note = Note::new("meeting");
        note.meeting_id = Some(meeting.id.clone());
        if let Err(e) = SqliteNoteRepository::new(conn).create(&note) {
            log::warn!("Failed to create the meeting notes row: {e}");
        }

        self.capture = Some(capture);
        self.meeting = Some(meeting.clone());
        self.meeting_t0 = now;
        self.audio_dir = Some(audio_dir);
        self.machine = Some(SessionMachine::new(
            meeting.id.clone(),
            now,
            MeetingPolicy::from_settings(&settings),
            true,          // mic live unless TrackUnavailable says otherwise
            !req.mic_only, // system track only in call mode
        ));

        // FR-009-02: discreet consent reminder on every start (opt-out).
        if settings.meeting_consent_reminder {
            self.emit_toast(ToastPayload {
                kind: "meeting_consent".to_string(),
                message: settings.meeting_consent_text.clone(),
                action: Some("copy_consent".to_string()),
            });
        }
        log::info!("Meeting {} started ({})", meeting.id, meeting.title);
        Ok(meeting)
    }

    /// Attach the shared mic stream + (optionally) the WASAPI loopback to a
    /// fresh [`MeetingCapture`] in `dir`.
    fn start_capture(
        &mut self,
        audio_dir: &Path,
        system_audio: bool,
        time_base: Instant,
        next_mic_index: u32,
        next_system_index: u32,
    ) -> CommandResult<MeetingCapture> {
        let (events_tx, events_rx) = mpsc::channel();
        let mic_tap = self.mic_tap();
        if mic_tap.is_none() {
            // `MicTap::new` never reached `subscribe_frame_consumer`, so the
            // capture pipeline can't report it — warn here instead.
            log::warn!("Meeting mic track unavailable: microphone stream is not open");
            if let Some(m) = self.machine.as_mut() {
                m.track_unavailable(Track::Mic);
            } else {
                self.emit_kind(ToastKind::MicUnavailable);
            }
        }
        let system = if system_audio {
            SystemSource::Default
        } else {
            SystemSource::Disabled
        };
        let capture = MeetingCapture::start_with_config(
            audio_dir.to_path_buf(),
            mic_tap,
            system,
            events_tx,
            CaptureConfig {
                time_base,
                next_mic_index,
                next_system_index,
                ..CaptureConfig::default()
            },
        )
        .map_err(|e| {
            CommandError::logged(
                CommandErrorCode::Internal,
                "Failed to start meeting capture",
                e,
            )
        })?;
        if !capture.is_capturing() {
            // Both tracks unavailable — recording nothing is worse than
            // refusing the start.
            return Err(CommandError::new(
                CommandErrorCode::AudioDevice,
                "No audio source is available for this meeting",
            ));
        }
        self.capture_rx = Some(events_rx);
        Ok(capture)
    }

    /// The meeting's mic tap on the shared stream (never opens the mic a
    /// second time — spec notas técnicas). Returns `None` when the stream
    /// cannot be opened (exclusive hold, no device).
    fn mic_tap(&self) -> Option<MicTap> {
        let rm = self.audio.as_ref()?;
        if let Err(e) = rm.start_microphone_stream() {
            log::warn!("Meeting could not open the microphone stream: {e}");
            return None;
        }
        let subscribe_rm = Arc::clone(rm);
        let unsubscribe_rm = Arc::clone(rm);
        Some(MicTap::new(
            move |cb| {
                subscribe_rm
                    .subscribe_frame_consumer(cb)
                    .map_err(|e| e.to_string())
            },
            move |id| {
                if let Err(e) = unsubscribe_rm.unsubscribe_frame_consumer(id) {
                    log::warn!("Failed to unsubscribe meeting mic tap: {e}");
                }
            },
        ))
    }

    fn drain_capture_events(&mut self) {
        let mut pending = Vec::new();
        if let Some(rx) = &self.capture_rx {
            while let Ok(event) = rx.try_recv() {
                pending.push(event);
            }
        }
        for event in pending {
            self.handle_capture_event(event);
        }
        self.flush();
    }

    fn handle_capture_event(&mut self, event: MeetingCaptureEvent) {
        let now = Instant::now();
        let Some(machine) = self.machine.as_mut() else {
            return;
        };
        match event {
            MeetingCaptureEvent::BlockSealed(block) => {
                let has_speech = self.speech.has_speech(&block);
                machine.block_sealed(block.track, has_speech, now);
            }
            MeetingCaptureEvent::TrackUnavailable { track, message } => {
                log::warn!("Meeting {:?} track unavailable: {message}", track);
                machine.track_unavailable(track);
            }
            MeetingCaptureEvent::WriteFailed { track, message } => {
                log::error!("Meeting {:?} block write failed: {message}", track);
                machine.write_failed(now);
            }
            MeetingCaptureEvent::System(SystemAudioEvent::Attached { after_gap, .. }) => {
                machine.system_attached(now, after_gap);
            }
            MeetingCaptureEvent::System(SystemAudioEvent::Detached { reason }) => {
                log::info!("Meeting system track detached: {reason:?}");
                machine.system_detached(now);
            }
        }
    }

    /// Drain the machine's journaled effects + `meeting://state` events.
    /// Effects can enqueue follow-ups (a failed resume re-warns), so drain
    /// until quiet with a small bound.
    fn flush(&mut self) {
        for _ in 0..4 {
            let mut effects = Vec::new();
            let mut events = Vec::new();
            if let Some(m) = self.machine.as_mut() {
                effects = m.take_effects();
                events = m.take_events();
            }
            for event in events {
                *self.snapshot.lock().unwrap_or_else(|e| e.into_inner()) = Some(event.clone());
                if let Err(e) = self.app.emit(MEETING_STATE_EVENT, &event) {
                    log::warn!("Failed to emit {MEETING_STATE_EVENT}: {e}");
                }
            }
            let had_work = !effects.is_empty();
            for effect in effects {
                self.run_effect(effect);
            }
            // A finished machine is dropped after its final effects ran.
            if self.machine.as_ref().is_some_and(|m| !m.is_active()) {
                self.machine = None;
                self.meeting = None;
                self.audio_dir = None;
            }
            if !had_work {
                break;
            }
        }
    }

    fn run_effect(&mut self, effect: Effect) {
        match effect {
            Effect::StartCapture => {
                // Resume path: same dir, same monotonic base, indices from
                // the on-disk scan.
                let Some(dir) = self.audio_dir.clone() else {
                    return;
                };
                let Some(meeting) = &self.meeting else {
                    return;
                };
                let capture_system = meeting.capture_system_audio;
                let (next_mic, next_system) = next_block_indices(&dir);
                match self.start_capture(
                    &dir,
                    capture_system,
                    self.meeting_t0,
                    next_mic,
                    next_system,
                ) {
                    Ok(capture) => self.capture = Some(capture),
                    Err(e) => {
                        log::error!("Meeting resume capture failed: {e}");
                        // Without any track the "resumed" meeting records
                        // silence — stop it safely instead.
                        if let Some(m) = self.machine.as_mut() {
                            m.stop(Instant::now(), StopReason::WriteFailures);
                        }
                    }
                }
            }
            Effect::StopCapture => {
                self.capture_rx = None;
                if let Some(capture) = self.capture.take() {
                    let summary = capture.stop();
                    log::debug!(
                        "Meeting capture stopped: {} mic / {} system blocks",
                        summary.mic.len(),
                        summary.system.len()
                    );
                }
            }
            Effect::WriteGapMarker {
                track,
                start_ms,
                end_ms,
            } => self.write_gap_marker(track, start_ms, end_ms),
            Effect::PersistStatus { status, error_code } => self.persist_status(status, error_code),
            Effect::Toast(kind) => self.emit_kind(kind),
            Effect::RequestProcessing => {
                let Some(meeting) = &self.meeting else {
                    return;
                };
                // T-067 seam: post-processing subscribes to this event.
                if let Err(e) = self.app.emit(
                    MEETING_PROCESS_REQUESTED_EVENT,
                    ProcessRequestedPayload {
                        meeting_id: meeting.id.clone(),
                    },
                ) {
                    log::warn!("Failed to emit {MEETING_PROCESS_REQUESTED_EVENT}: {e}");
                }
            }
            Effect::Indicator(active) => self.update_indicator(active),
        }
    }

    fn write_gap_marker(&mut self, track: Track, start_ms: i64, end_ms: i64) {
        let Some(meeting) = &self.meeting else {
            return;
        };
        let Some(conn) = self.conn.as_ref() else {
            return;
        };
        let mut segment = MeetingSegment::new(&meeting.id, track.label(), start_ms, end_ms, "");
        segment.kind = "gap_marker".to_string();
        if let Err(e) = SqliteMeetingSegmentRepository::new(conn).create(&segment) {
            log::warn!("Failed to persist meeting gap marker: {e}");
        }
    }

    fn persist_status(&mut self, status: &'static str, error_code: Option<&'static str>) {
        let Some(conn) = self.conn.as_ref() else {
            return;
        };
        let Some(meeting) = &self.meeting else {
            return;
        };
        let repo = SqliteMeetingRepository::new(conn);
        // Full update so `ended_at` is stamped exactly once when the meeting
        // leaves the recording states (`processing`/`error` are terminal for
        // the capture) — `set_status` only stamps a hard-coded subset.
        let row = match repo.get(&meeting.id) {
            Ok(Some(row)) => row,
            Ok(None) => meeting.clone(),
            Err(e) => {
                log::warn!("Failed to reload meeting for status update: {e}");
                meeting.clone()
            }
        };
        let mut row = row;
        row.status = status.to_string();
        row.error_code = error_code.map(str::to_string);
        if row.ended_at.is_none() && matches!(status, "processing" | "error") {
            row.ended_at = Some(chrono::Utc::now().timestamp());
        }
        if let Err(e) = repo.update(&row) {
            log::warn!("Failed to persist meeting status '{status}': {e}");
        }
    }

    /// FR-009-07: tray icon + "Stop Meeting" row + Flow Bar pill. The
    /// indicator cannot be user-hidden; suppression settings may still unmap
    /// the overlay window itself (that's the Flow Bar's contract, not the
    /// indicator's).
    fn update_indicator(&mut self, active: bool) {
        MEETING_ACTIVE.store(active, Ordering::SeqCst);
        crate::tray::update_tray_menu(&self.app);
        if active {
            crate::overlay::show_recording_overlay(&self.app);
        } else {
            // Don't clobber a live dictation pill: the overlay is shared.
            let dictation_active = self
                .app
                .try_state::<TranscriptionCoordinator>()
                .and_then(|c| c.current_session())
                .is_some();
            if !dictation_active {
                crate::overlay::hide_recording_overlay(&self.app);
            }
        }
    }

    fn emit_kind(&mut self, kind: ToastKind) {
        let lang = get_settings(&self.app).app_language;
        self.emit_toast(ToastPayload {
            kind: kind.kind().to_string(),
            message: toast_message(kind, &lang),
            action: kind.action().map(str::to_string),
        });
    }

    fn emit_toast(&mut self, payload: ToastPayload) {
        if let Err(e) = self.app.emit(TOAST_SHOW_EVENT, &payload) {
            log::warn!("Failed to emit {TOAST_SHOW_EVENT}: {e}");
        }
    }

    fn conn(&mut self) -> CommandResult<&Connection> {
        if self.conn.is_none() {
            self.conn = Some(open_session_db(&self.app)?);
        }
        match self.conn.as_ref() {
            Some(conn) => Ok(conn),
            None => Err(CommandError::new(
                CommandErrorCode::Internal,
                "Meeting database is unavailable",
            )),
        }
    }
}
