//! Meeting post-processing pipeline (T-067; FR-009-16..22, AC-009-01/06).
//!
//! Triggered by `meeting://process-requested` (emitted by the session worker
//! when a meeting enters `processing`) and by the `meeting_retry_processing`
//! / `meeting_regenerate_summary` commands. A single worker thread serializes
//! all jobs so two meetings — or a full pass and a regeneration — never run
//! concurrently.
//!
//! Steps (FR-009-16), each announced on `meeting://progress`:
//!
//! 1. Transcribe pending blocks ([`transcribe`]) — sealed WAVs under
//!    `audio/meetings/<id>/` that no `speech` segment covers yet are
//!    VAD-sliced into ≤ 30 s regions and sent to the meeting STT model.
//!    Track/block-level failures keep the partial transcript (warn +
//!    continue); only "no model at all" is fatal.
//! 2. Refine with a bigger model — **P1, skipped** (progress still emitted so
//!    the step indices match the spec).
//! 3. `system`-track diarization — **P1, skipped** (same).
//! 4. Summary ([`summarize`]) — `llm::router::complete_for_purpose` under the
//!    selected `summary_templates` prompt, with map-reduce (~20 min windows +
//!    consolidation) past the context budget (FR-009-18). "Minhas notas" is
//!    injected as priority context and **never** written back (FR-009-19).
//! 5. Suggested title — LLM `Title` call, applied only while the row still
//!    carries the `default_title` placeholder so user edits survive
//!    (FR-009-20). Fallback: keep `"<App> · <data>"`.
//!
//! End states: `ready` (with `summary_status` `ready`/`disabled`) or
//! `error` + `toast://show` `meeting_error` → `meeting_retry_processing`
//! re-runs the whole pipeline (FR-009-22; audio stays in place either way).
//!
//! Never log transcript/notes/summary text or prompts — ids, lengths and
//! timings only.

mod coverage;
mod prompt;
mod summarize;
mod transcribe;

use std::sync::mpsc::{self, Sender};

use rusqlite::Connection;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Listener, Manager};
use tokio::sync::oneshot;

use crate::commands::{CommandError, CommandErrorCode, CommandResult};
use crate::db::meetings::{Meeting, MeetingRepository, SqliteMeetingRepository};
use crate::meeting::session::{
    open_session_db, MeetingStateEvent, ProcessRequestedPayload, ToastPayload,
    MEETING_PROCESS_REQUESTED_EVENT, MEETING_STATE_EVENT, TOAST_SHOW_EVENT,
};
use crate::settings::get_settings;

/// `meeting://progress` — contracts.md §5 (`{ meeting_id, step, pct }`) plus
/// the step bookkeeping T-066's progress UI wants (`step_index`,
/// `total_steps`, `percent` alias).
pub const MEETING_PROGRESS_EVENT: &str = "meeting://progress";
/// `meeting://segment` — a freshly persisted speech row (contracts.md §5);
/// lets an open meeting window stream the post-processing transcription.
pub const MEETING_SEGMENT_EVENT: &str = "meeting://segment";

/// Step names in FR-009-16 order — 1-based `step_index` matches the spec.
pub(crate) const STEP_TRANSCRIBE: &str = "transcribe";
pub(crate) const STEP_REFINE: &str = "refine";
pub(crate) const STEP_DIARIZE: &str = "diarize";
pub(crate) const STEP_SUMMARY: &str = "summary";
pub(crate) const STEP_TITLE: &str = "title";
pub(crate) const STEP_DONE: &str = "done";
pub(crate) const TOTAL_STEPS: u32 = 5;

/// Progress milestones — a monotone ladder over the 5 spec steps. Steps 2–3
/// are P1 (skipped) but still consume their share so the bar never retreats.
pub(crate) const PCT_TRANSCRIBE_START: u32 = 5;
pub(crate) const PCT_TRANSCRIBE_DONE: u32 = 55;
pub(crate) const PCT_REFINE_DONE: u32 = 60;
pub(crate) const PCT_DIARIZE_DONE: u32 = 65;
pub(crate) const PCT_SUMMARY_DONE: u32 = 90;
pub(crate) const PCT_TITLE_DONE: u32 = 97;

/// `meeting://progress` payload.
#[derive(Clone, Debug, Serialize)]
pub struct MeetingProgress {
    pub meeting_id: String,
    /// FR-009-16 step name ("transcribe" | "refine" | "diarize" | "summary" |
    /// "title" | "done").
    pub step: String,
    /// 1-based index in spec order; `done` carries `total_steps`.
    pub step_index: u32,
    pub total_steps: u32,
    /// Contract name (contracts.md §5) — 0..100.
    pub pct: u32,
    /// Same value as `pct` — the field name the T-066 window binds to.
    pub percent: u32,
}

impl MeetingProgress {
    pub(crate) fn new(meeting_id: &str, step: &str, step_index: u32, pct: u32) -> Self {
        let pct = pct.min(100);
        Self {
            meeting_id: meeting_id.to_string(),
            step: step.to_string(),
            step_index,
            total_steps: TOTAL_STEPS,
            pct,
            percent: pct,
        }
    }
}

/// What a queued job asks the worker to run.
pub(crate) enum Job {
    /// Steps 1–5 (`meeting://process-requested` seam and `meeting_retry_processing`).
    Full {
        meeting_id: String,
        reply: Option<oneshot::Sender<CommandResult<()>>>,
    },
    /// Steps 4–5 only (`meeting_regenerate_summary`, FR-009-20).
    SummaryOnly {
        meeting_id: String,
        reply: oneshot::Sender<CommandResult<()>>,
    },
}

/// Outcome of the summary step — drives `meetings.summary_status` (and, for
/// the full pipeline, the meeting-level status).
#[derive(Debug)]
pub(crate) enum SummaryOutcome {
    Ready,
    /// FR-009-21: no usable LLM configuration — the meeting still reaches
    /// `ready` with transcript + notes.
    Disabled,
    /// Nothing to summarize (zero speech content) — `disabled` row-wise but
    /// no configuration warning is warranted.
    Empty,
    /// An LLM failure that survived `complete_with_retry` — retriable.
    Failed(CommandError),
}

/// Handle to the post-processing worker. Managed as Tauri state; the
/// `meeting://process-requested` listener and the commands both enqueue onto
/// the same channel. `Clone` so commands can clone the managed state.
#[derive(Clone)]
pub struct MeetingPostProcessor {
    tx: Sender<Job>,
}

impl MeetingPostProcessor {
    /// Spawn the `meeting-postprocess` worker thread.
    pub fn new(app: AppHandle) -> Self {
        let (tx, rx) = mpsc::channel::<Job>();
        let spawned = std::thread::Builder::new()
            .name("meeting-postprocess".to_string())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    Worker::new(app).run(rx);
                }));
                if let Err(e) = result {
                    log::error!("Meeting post-processing worker panicked: {e:?}");
                }
            });
        if let Err(e) = spawned {
            log::error!("Failed to spawn meeting post-processing worker: {e}");
        }
        Self { tx }
    }

    /// Queue a full pass without a reply channel — the session's
    /// `meeting://process-requested` seam.
    pub fn enqueue_process(&self, meeting_id: String) {
        if let Err(e) = self.tx.send(Job::Full {
            meeting_id,
            reply: None,
        }) {
            log::error!("Meeting post-processing channel closed: {e}");
        }
    }

    /// Queue a job whose result the caller awaits (the `meeting_*` commands).
    pub(crate) fn request(&self, job: Job) -> CommandResult<()> {
        self.tx.send(job).map_err(|_| {
            CommandError::logged(
                CommandErrorCode::Internal,
                "Meeting post-processing is unavailable",
                "worker channel closed",
            )
        })
    }
}

/// Wire the session's `meeting://process-requested` event to the pipeline.
/// Called once from `initialize_core_logic` after the session manager is up.
pub fn init(app: &AppHandle) {
    let processor = MeetingPostProcessor::new(app.clone());
    app.manage(processor);
    let handle = app.clone();
    app.listen(
        MEETING_PROCESS_REQUESTED_EVENT,
        move |event| match serde_json::from_str::<ProcessRequestedPayload>(event.payload()) {
            Ok(payload) => {
                if let Some(processor) = handle.try_state::<MeetingPostProcessor>() {
                    processor.enqueue_process(payload.meeting_id);
                }
            }
            Err(e) => {
                log::warn!("Ignoring malformed {MEETING_PROCESS_REQUESTED_EVENT} payload: {e}")
            }
        },
    );
}

pub(crate) struct Worker {
    pub(crate) app: AppHandle,
    conn: Option<Connection>,
}

impl Worker {
    fn new(app: AppHandle) -> Self {
        Self { app, conn: None }
    }

    fn run(&mut self, rx: mpsc::Receiver<Job>) {
        for job in rx {
            match job {
                Job::Full { meeting_id, reply } => {
                    let result = self.run_full(&meeting_id);
                    if let Some(reply) = reply {
                        let _ = reply.send(result);
                    }
                }
                Job::SummaryOnly { meeting_id, reply } => {
                    let _ = reply.send(self.run_summary_only(&meeting_id));
                }
            }
        }
        log::debug!("Meeting post-processing worker exited");
    }

    // -- shared plumbing ----------------------------------------------------

    pub(crate) fn conn(&mut self) -> CommandResult<&Connection> {
        if self.conn.is_none() {
            self.conn = Some(open_session_db(&self.app)?);
        }
        self.conn.as_ref().ok_or_else(|| {
            CommandError::new(
                CommandErrorCode::Internal,
                "Meeting database is unavailable",
            )
        })
    }

    /// Borrow the db connection without initializing it — `None` when it has
    /// never been opened (steps that can degrade gracefully use this).
    pub(crate) fn conn_opt(&self) -> Option<&Connection> {
        self.conn.as_ref()
    }

    pub(crate) fn meeting(&mut self, meeting_id: &str) -> CommandResult<Meeting> {
        let found = SqliteMeetingRepository::new(self.conn()?)
            .get(meeting_id)
            .map_err(|e| {
                CommandError::logged(CommandErrorCode::Internal, "Failed to load the meeting", e)
            })?;
        found.ok_or_else(|| CommandError::new(CommandErrorCode::NotFound, "Meeting not found"))
    }

    pub(crate) fn emit_progress(&self, meeting_id: &str, step: &str, step_index: u32, pct: u32) {
        if let Err(e) = self.app.emit(
            MEETING_PROGRESS_EVENT,
            MeetingProgress::new(meeting_id, step, step_index, pct),
        ) {
            log::warn!("Failed to emit {MEETING_PROGRESS_EVENT}: {e}");
        }
    }

    pub(crate) fn emit_segment(&self, segment: &crate::db::meetings::MeetingSegment) {
        if let Err(e) = self.app.emit(MEETING_SEGMENT_EVENT, segment) {
            log::warn!("Failed to emit {MEETING_SEGMENT_EVENT}: {e}");
        }
    }

    fn emit_state(&self, meeting_id: &str, status: &str, elapsed_ms: u64) {
        if let Err(e) = self.app.emit(
            MEETING_STATE_EVENT,
            MeetingStateEvent {
                meeting_id: meeting_id.to_string(),
                status: status.to_string(),
                elapsed_ms,
            },
        ) {
            log::warn!("Failed to emit {MEETING_STATE_EVENT}: {e}");
        }
    }

    fn emit_toast(&self, kind: &str, message: String, action: Option<&str>) {
        self.emit_toast_for(kind, message, action, None);
    }

    /// `meeting_id` rides the payload so action buttons like
    /// `retry_processing` know which meeting they act on.
    fn emit_toast_for(
        &self,
        kind: &str,
        message: String,
        action: Option<&str>,
        meeting_id: Option<&str>,
    ) {
        if let Err(e) = self.app.emit(
            TOAST_SHOW_EVENT,
            ToastPayload {
                kind: kind.to_string(),
                message,
                action: action.map(str::to_string),
                meeting_id: meeting_id.map(str::to_string),
            },
        ) {
            log::warn!("Failed to emit {TOAST_SHOW_EVENT}: {e}");
        }
    }

    /// Localized toast copy for pipeline warnings (same convention as
    /// `session::toast_message`: `app_language` is normalized to "pt-BR"|"en").
    fn toast_message(&self, kind: &str) -> String {
        let pt = get_settings(&self.app).app_language == "pt-BR";
        match (kind, pt) {
            ("meeting_summary_disabled", true) => {
                "Resumo indisponível — configure um provedor de resumo (LLM) nas configurações."
                    .to_string()
            }
            ("meeting_summary_disabled", false) => {
                "Summary unavailable — configure a summary (LLM) provider in settings.".to_string()
            }
            ("meeting_error", true) => {
                "Falha ao processar a reunião — o áudio foi preservado. Tente novamente."
                    .to_string()
            }
            ("meeting_error", false) => {
                "Failed to process the meeting — the audio was preserved. Try again.".to_string()
            }
            _ => "Meeting processing finished with warnings.".to_string(),
        }
    }

    pub(crate) fn elapsed_ms(meeting: &Meeting) -> u64 {
        let end = meeting
            .ended_at
            .unwrap_or_else(|| chrono::Utc::now().timestamp());
        (end - meeting.started_at).max(0) as u64 * 1000
    }

    // -- the two pipeline entry points ---------------------------------------

    /// FR-009-16 full pass: steps 1–5. Failures land on `status='error'`
    /// (+ `meeting_error` toast); audio is never touched (FR-009-22).
    fn run_full(&mut self, meeting_id: &str) -> CommandResult<()> {
        let meeting = self.meeting(meeting_id)?;
        if matches!(meeting.status.as_str(), "recording" | "paused") {
            return Err(CommandError::new(
                CommandErrorCode::InvalidInput,
                "The meeting is still recording",
            ));
        }
        // A retried meeting may still show 'error'/'recovered': mark it
        // 'processing' so the window sees the pass in flight.
        if meeting.status != "processing" {
            self.persist_status(meeting_id, "processing", None);
            self.emit_state(meeting_id, "processing", Self::elapsed_ms(&meeting));
        }

        self.emit_progress(meeting_id, STEP_TRANSCRIBE, 1, PCT_TRANSCRIBE_START);
        if let Err(e) = self.transcribe_pending(&meeting) {
            return self.fail(meeting_id, "transcription_failed", e);
        }

        // Steps 2–3 (refine / diarization) are P1 — see module docs.
        self.emit_progress(meeting_id, STEP_REFINE, 2, PCT_REFINE_DONE);
        self.emit_progress(meeting_id, STEP_DIARIZE, 3, PCT_DIARIZE_DONE);

        match self.summarize(&meeting) {
            SummaryOutcome::Failed(e) => {
                return self.fail(meeting_id, "summary_failed", e);
            }
            SummaryOutcome::Disabled => {
                self.emit_toast(
                    "meeting_summary_disabled",
                    self.toast_message("meeting_summary_disabled"),
                    Some("open_summary_settings"),
                );
            }
            SummaryOutcome::Ready | SummaryOutcome::Empty => {}
        }
        self.emit_progress(meeting_id, STEP_SUMMARY, 4, PCT_SUMMARY_DONE);

        let meeting = self.meeting(meeting_id).unwrap_or(meeting);
        self.suggest_title(&meeting);
        self.emit_progress(meeting_id, STEP_TITLE, 5, PCT_TITLE_DONE);

        self.persist_status(meeting_id, "ready", None);
        self.emit_state(meeting_id, "ready", Self::elapsed_ms(&meeting));
        self.emit_progress(meeting_id, STEP_DONE, TOTAL_STEPS, 100);
        log::info!("Meeting {meeting_id} post-processing finished");
        Ok(())
    }

    /// FR-009-20: steps 4–5 only. The meeting row keeps its status; only
    /// `summary_status` moves. Errors are reported through the reply channel
    /// (and `summary_status='error'`) instead of failing the meeting.
    fn run_summary_only(&mut self, meeting_id: &str) -> CommandResult<()> {
        let meeting = self.meeting(meeting_id)?;
        if matches!(meeting.status.as_str(), "recording" | "paused") {
            return Err(CommandError::new(
                CommandErrorCode::InvalidInput,
                "The meeting is still recording",
            ));
        }
        self.persist_summary_status(meeting_id, "pending");
        self.emit_progress(meeting_id, STEP_SUMMARY, 4, PCT_DIARIZE_DONE);
        match self.summarize(&meeting) {
            SummaryOutcome::Ready => {}
            SummaryOutcome::Empty => {
                return Err(CommandError::new(
                    CommandErrorCode::InvalidInput,
                    "There is no transcript to summarize",
                ));
            }
            SummaryOutcome::Disabled => {
                return Err(self.disabled_error());
            }
            SummaryOutcome::Failed(e) => return Err(e),
        }
        self.emit_progress(meeting_id, STEP_SUMMARY, 4, PCT_SUMMARY_DONE);
        let meeting = self.meeting(meeting_id).unwrap_or(meeting);
        self.suggest_title(&meeting);
        self.emit_progress(meeting_id, STEP_TITLE, 5, PCT_TITLE_DONE);
        self.emit_progress(meeting_id, STEP_DONE, TOTAL_STEPS, 100);
        Ok(())
    }

    /// FR-009-22: unrecoverable step failure → `error` + retry toast.
    /// Returns the failing `CommandError` so awaiting callers see it too.
    fn fail(
        &mut self,
        meeting_id: &str,
        error_code: &'static str,
        err: CommandError,
    ) -> CommandResult<()> {
        log::warn!("Meeting {meeting_id} processing failed ({error_code})");
        self.persist_status(meeting_id, "error", Some(error_code));
        self.persist_summary_status(meeting_id, "error");
        if let Ok(meeting) = self.meeting(meeting_id) {
            self.emit_state(meeting_id, "error", Self::elapsed_ms(&meeting));
        }
        self.emit_toast_for(
            "meeting_error",
            self.toast_message("meeting_error"),
            Some("retry_processing"),
            Some(meeting_id),
        );
        Err(err)
    }

    pub(crate) fn persist_status(
        &mut self,
        meeting_id: &str,
        status: &str,
        error_code: Option<&str>,
    ) {
        if let Some(conn) = self.conn_opt() {
            if let Err(e) =
                SqliteMeetingRepository::new(conn).set_status(meeting_id, status, error_code)
            {
                log::warn!("Failed to persist meeting status '{status}': {e}");
            }
        }
    }

    pub(crate) fn persist_summary_status(&mut self, meeting_id: &str, status: &str) {
        if let Some(conn) = self.conn_opt() {
            if let Err(e) =
                SqliteMeetingRepository::new(conn).set_summary_status(meeting_id, status)
            {
                log::warn!("Failed to persist meeting summary_status '{status}': {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_payload_matches_the_contract() {
        let p = MeetingProgress::new("m1", STEP_SUMMARY, 4, 90);
        let json = serde_json::to_value(&p).expect("serialize");
        assert_eq!(
            json,
            serde_json::json!({
                "meeting_id": "m1",
                "step": "summary",
                "step_index": 4,
                "total_steps": 5,
                "pct": 90,
                "percent": 90,
            })
        );
    }

    #[test]
    fn progress_pct_is_capped_at_100() {
        assert_eq!(MeetingProgress::new("m", STEP_DONE, 5, 500).pct, 100);
    }
}
