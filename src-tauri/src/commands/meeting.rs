//! Meeting session commands (T-064; FR-009-01/02, FR-009-06..09).
//!
//! Lifecycle calls are thin forwards to [`MeetingSessionManager`] — the
//! worker thread owns the single active meeting and every ordering decision.
//! Read commands (`meeting_get`/`meeting_list`/`meeting_current`) and the
//! consent pair (`meeting_consent_accept`/`meeting_consent_copy`) don't need
//! the worker and answer straight from SQLite / settings.
//!
//! `meeting_delete` refuses the *active* meeting — stopping it first keeps
//! the on-disk blocks consistent — and only ever removes an audio directory
//! inside `audio/meetings/`.

use serde::Serialize;
use specta::Type;
use tauri::{AppHandle, Manager};
use tokio::sync::oneshot;

use crate::db::meetings::{
    Meeting, MeetingRepository, MeetingSegment, MeetingSegmentRepository, SqliteMeetingRepository,
    SqliteMeetingSegmentRepository,
};
use crate::db::notes::{Note, NoteRepository, SqliteNoteRepository};
use crate::db::summary_templates::{
    SqliteSummaryTemplateRepository, SummaryTemplate, SummaryTemplateRepository,
};
use crate::meeting::markdown::{markdown_labels, render_meeting_markdown};
use crate::meeting::postprocess::{Job, MeetingPostProcessor};
use crate::meeting::session::{
    meeting_recording_active, open_session_db, remove_meeting_audio_dir, MeetingSessionManager,
    MeetingStateEvent, StartRequest,
};
use crate::portable::app_data_dir;
use crate::settings::{get_settings, write_settings};

use super::{CommandError, CommandErrorCode, CommandResult};

fn manager(app: &AppHandle) -> CommandResult<MeetingSessionManager> {
    app.try_state::<MeetingSessionManager>()
        .map(|s| s.inner().clone())
        .ok_or_else(|| {
            CommandError::logged(
                CommandErrorCode::Internal,
                "Meeting session is unavailable",
                "MeetingSessionManager is not managed",
            )
        })
}

/// FR-009-01: start the meeting. `mic_only` selects "Presencial" (in-person,
/// mic track only); the default is "Chamada no computador" (mic + system
/// loopback). Detector/Flow Bar starts go through the same gate.
#[tauri::command]
#[specta::specta]
pub fn meeting_start(app: AppHandle, mic_only: Option<bool>) -> CommandResult<Meeting> {
    let mut req = StartRequest::manual();
    req.mic_only = mic_only.unwrap_or(false);
    req.detection = if req.mic_only {
        "in_person".to_string()
    } else {
        "manual".to_string()
    };
    manager(&app)?.request_start(req)
}

/// FR-009-06: pause both tracks and stamp the meeting `paused`.
#[tauri::command]
#[specta::specta]
pub fn meeting_pause(app: AppHandle) -> CommandResult<()> {
    manager(&app)?.request_pause()
}

/// FR-009-06: resume — a `gap_marker` covers the pause span and block
/// numbering continues from the on-disk files.
#[tauri::command]
#[specta::specta]
pub fn meeting_resume(app: AppHandle) -> CommandResult<()> {
    manager(&app)?.request_resume()
}

/// FR-009-06: stop, seal the in-flight blocks and hand off to processing.
#[tauri::command]
#[specta::specta]
pub fn meeting_stop(app: AppHandle) -> CommandResult<()> {
    manager(&app)?.request_stop()
}

/// FR-009-08: the "Estender 30 min" toast action.
#[tauri::command]
#[specta::specta]
pub fn meeting_extend_30(app: AppHandle) -> CommandResult<()> {
    manager(&app)?.request_extend()
}

/// FR-009-09: answer the silence check-in — `keep_recording: true` for
/// "Continuar", `false` for "Parar" (stops and processes the meeting).
#[tauri::command]
#[specta::specta]
pub fn meeting_checkin_respond(app: AppHandle, keep_recording: bool) -> CommandResult<()> {
    manager(&app)?.request_checkin_respond(keep_recording)
}

/// FR-008-14 (T-069): "Continuar gravando" on the auto-stop toast — cancels
/// the pending 15 s auto-stop; the meeting keeps recording until the user
/// stops it (or another rule fires). Idempotent while a meeting is active.
#[tauri::command]
#[specta::specta]
pub fn meeting_continue_recording(app: AppHandle) -> CommandResult<()> {
    manager(&app)?.request_continue_recording()
}

/// Latest `meeting://state` snapshot — lets a freshly mounted frontend catch
/// up without waiting for the next tick.
#[tauri::command]
#[specta::specta]
pub fn meeting_current(app: AppHandle) -> CommandResult<Option<MeetingStateEvent>> {
    Ok(manager(&app)?.current())
}

/// Upper bound for a user-edited meeting title (FR-009-12 inline rename).
const MEETING_TITLE_MAX_CHARS: usize = 160;

/// Hydration payload for the meeting window (T-066): the row plus everything
/// its three tabs render — transcript segments and the "Minhas notas" body.
/// `meeting://segment` events append live on top of this snapshot.
#[derive(Clone, Debug, Serialize, Type)]
pub struct MeetingDetail {
    pub meeting: Meeting,
    /// Ordered by `start_ms`.
    pub segments: Vec<MeetingSegment>,
    /// The meeting note row's markdown body ("" when the row is missing —
    /// e.g. a meeting that predates the notes-row creation).
    pub notes_md: String,
}

/// One meeting with its segments and notes body (meeting window hydration).
#[tauri::command]
#[specta::specta]
pub fn meeting_get(app: AppHandle, id: String) -> CommandResult<Option<MeetingDetail>> {
    let conn = open_session_db(&app)?;
    let Some(meeting) = SqliteMeetingRepository::new(&conn).get(&id).map_err(|e| {
        CommandError::logged(CommandErrorCode::Internal, "Failed to load the meeting", e)
    })?
    else {
        return Ok(None);
    };
    let segments = SqliteMeetingSegmentRepository::new(&conn)
        .list_by_meeting(&id)
        .map_err(|e| {
            CommandError::logged(
                CommandErrorCode::Internal,
                "Failed to load the meeting transcript",
                e,
            )
        })?;
    let notes_md = SqliteNoteRepository::new(&conn)
        .list_by_meeting(&id)
        .map_err(|e| {
            CommandError::logged(
                CommandErrorCode::Internal,
                "Failed to load meeting notes",
                e,
            )
        })?
        .into_iter()
        .find(|n| n.source == "meeting")
        .map(|n| n.body_md)
        .unwrap_or_default();
    Ok(Some(MeetingDetail {
        meeting,
        segments,
        notes_md,
    }))
}

/// FR-009-12: inline title edit. Rejects blank/oversized input at the
/// boundary and retitles the native window when it is showing this meeting.
#[tauri::command]
#[specta::specta]
pub fn meeting_rename(app: AppHandle, id: String, title: String) -> CommandResult<Meeting> {
    let title = title.trim();
    if title.is_empty() {
        return Err(CommandError::new(
            CommandErrorCode::InvalidInput,
            "The meeting title cannot be empty",
        ));
    }
    if title.chars().count() > MEETING_TITLE_MAX_CHARS {
        return Err(CommandError::new(
            CommandErrorCode::InvalidInput,
            "The meeting title is too long",
        ));
    }
    let conn = open_session_db(&app)?;
    let repo = SqliteMeetingRepository::new(&conn);
    let mut meeting = repo
        .get(&id)
        .map_err(|e| {
            CommandError::logged(CommandErrorCode::Internal, "Failed to load the meeting", e)
        })?
        .ok_or_else(|| CommandError::new(CommandErrorCode::NotFound, "Meeting not found"))?;
    meeting.title = title.to_string();
    repo.update(&meeting).map_err(|e| {
        CommandError::logged(
            CommandErrorCode::Internal,
            "Failed to rename the meeting",
            e,
        )
    })?;
    crate::meeting_window::retitle_if_shown(&app, &id, &meeting.title);
    Ok(meeting)
}

/// FR-009-13 "Minhas notas" debounced autosave. Writes ONLY the note row's
/// `body_md` — FR-009-19: this text is user-owned and never touched by the
/// summary pipeline. The row is created at meeting start (T-064); it is
/// recreated defensively when absent (e.g. meetings that predate it).
#[tauri::command]
#[specta::specta]
pub fn meeting_notes_update(app: AppHandle, id: String, body_md: String) -> CommandResult<()> {
    let conn = open_session_db(&app)?;
    // FK integrity: a note can't reference a meeting that does not exist.
    let meeting = SqliteMeetingRepository::new(&conn)
        .get(&id)
        .map_err(|e| {
            CommandError::logged(CommandErrorCode::Internal, "Failed to load the meeting", e)
        })?
        .ok_or_else(|| CommandError::new(CommandErrorCode::NotFound, "Meeting not found"))?;
    let notes = SqliteNoteRepository::new(&conn);
    match notes
        .list_by_meeting(&id)
        .map_err(|e| {
            CommandError::logged(
                CommandErrorCode::Internal,
                "Failed to load meeting notes",
                e,
            )
        })?
        .into_iter()
        .find(|n| n.source == "meeting")
    {
        Some(mut note) => {
            note.body_md = body_md;
            notes.update(&note).map_err(|e| {
                CommandError::logged(CommandErrorCode::Internal, "Failed to save notes", e)
            })?;
        }
        None => {
            let mut note = Note::new("meeting");
            note.meeting_id = Some(id);
            note.title = meeting.title;
            note.body_md = body_md;
            notes.create(&note).map_err(|e| {
                CommandError::logged(CommandErrorCode::Internal, "Failed to save notes", e)
            })?;
        }
    }
    Ok(())
}

/// FR-009-13: "Resumo" is user-editable after generation — writes only
/// `summary_md`, never the notes row (FR-009-19).
#[tauri::command]
#[specta::specta]
pub fn meeting_summary_update(app: AppHandle, id: String, summary_md: String) -> CommandResult<()> {
    let conn = open_session_db(&app)?;
    let repo = SqliteMeetingRepository::new(&conn);
    let mut meeting = repo
        .get(&id)
        .map_err(|e| {
            CommandError::logged(CommandErrorCode::Internal, "Failed to load the meeting", e)
        })?
        .ok_or_else(|| CommandError::new(CommandErrorCode::NotFound, "Meeting not found"))?;
    meeting.summary_md = Some(summary_md);
    repo.update(&meeting).map_err(|e| {
        CommandError::logged(CommandErrorCode::Internal, "Failed to save the summary", e)
    })?;
    Ok(())
}

/// FR-009-14: open (or focus) the meeting window. `meeting_id` omitted → the
/// active session, else the most recent meeting.
#[tauri::command]
#[specta::specta]
pub fn meeting_window_open(app: AppHandle, meeting_id: Option<String>) -> CommandResult<()> {
    crate::meeting_window::open(&app, meeting_id)
}

/// Every meeting, newest first (FR-009-13 "Minhas notas" list).
#[tauri::command]
#[specta::specta]
pub fn meeting_list(app: AppHandle) -> CommandResult<Vec<Meeting>> {
    let conn = open_session_db(&app)?;
    SqliteMeetingRepository::new(&conn)
        .list()
        .map_err(|e| CommandError::logged(CommandErrorCode::Internal, "Failed to list meetings", e))
}

/// Segments of a meeting in meeting-clock order (T-065/T-066): `speech`,
/// `dictation_marker` and `gap_marker` rows. Excluded mic speech
/// (FR-009-10) is returned — the UI hides `excluded` rows and shows the
/// covering marker. Also the hydration path for listeners that missed
/// `meeting://segment` events.
#[tauri::command]
#[specta::specta]
pub fn meeting_segments(app: AppHandle, meeting_id: String) -> CommandResult<Vec<MeetingSegment>> {
    let conn = open_session_db(&app)?;
    SqliteMeetingSegmentRepository::new(&conn)
        .list_by_meeting(&meeting_id)
        .map_err(|e| {
            CommandError::logged(
                CommandErrorCode::Internal,
                "Failed to load the meeting segments",
                e,
            )
        })
}

/// FR-009-25 (T-068): full-text search over title, summary, "Minhas notas"
/// and transcript. `query` is raw user input — `fts_match_query` strips
/// everything FTS5 would choke on; an empty/unindexable query lists all
/// meetings (same result as `meeting_list`).
#[tauri::command]
#[specta::specta]
pub fn meeting_search(app: AppHandle, query: String) -> CommandResult<Vec<Meeting>> {
    let conn = open_session_db(&app)?;
    let repo = SqliteMeetingRepository::new(&conn);
    let fail = |e| CommandError::logged(CommandErrorCode::Internal, "Failed to search meetings", e);
    match crate::db::fts_match_query(&query) {
        Some(fts_query) => repo.search(&fts_query).map_err(fail),
        None => repo.list().map_err(fail),
    }
}

/// FR-009-23 / AC-009-07 (T-068): assemble the meeting as a Markdown
/// document — title, date, duration, app, Minhas notas, Resumo and the
/// `[mm:ss] Falante: texto` transcript. Section and speaker labels follow
/// `settings.app_language`; the frontend copies the returned string to the
/// clipboard.
#[tauri::command]
#[specta::specta]
pub fn meeting_export_markdown(app: AppHandle, id: String) -> CommandResult<String> {
    let conn = open_session_db(&app)?;
    let meeting = SqliteMeetingRepository::new(&conn)
        .get(&id)
        .map_err(|e| {
            CommandError::logged(CommandErrorCode::Internal, "Failed to load the meeting", e)
        })?
        .ok_or_else(|| CommandError::new(CommandErrorCode::NotFound, "Meeting not found"))?;
    let notes = SqliteNoteRepository::new(&conn)
        .list_by_meeting(&id)
        .map_err(|e| {
            CommandError::logged(CommandErrorCode::Internal, "Failed to load the notes", e)
        })?;
    let segments = SqliteMeetingSegmentRepository::new(&conn)
        .list_by_meeting(&id)
        .map_err(|e| {
            CommandError::logged(
                CommandErrorCode::Internal,
                "Failed to load the transcript",
                e,
            )
        })?;

    // "Minhas notas" = the meeting note bodies concatenated (usually one
    // row); kept verbatim — FR-009-19 says notes are never rewritten.
    let notes_md = notes
        .iter()
        .map(|n| n.body_md.trim())
        .filter(|b| !b.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");

    let labels = markdown_labels(&get_settings(&app).app_language);
    let started_at_label = chrono::DateTime::from_timestamp(meeting.started_at, 0)
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "—".to_string());

    Ok(render_meeting_markdown(
        &meeting,
        Some(notes_md.as_str()).filter(|s| !s.is_empty()),
        &segments,
        &labels,
        &started_at_label,
    ))
}

/// Delete a meeting row (segments/notes cascade) plus its audio blocks —
/// the dir is only removed when it sits inside `audio/meetings/`.
#[tauri::command]
#[specta::specta]
pub fn meeting_delete(app: AppHandle, id: String) -> CommandResult<()> {
    // Meeting ids are UUIDs minted server-side; rejecting anything else here
    // keeps a forged id ("../dictations") from ever reaching the filesystem
    // delete — the UUID check in `remove_meeting_audio_dir` is the backstop.
    if uuid::Uuid::parse_str(&id).is_err() {
        return Err(CommandError::new(
            CommandErrorCode::InvalidInput,
            "Invalid meeting id",
        ));
    }
    if meeting_recording_active()
        && manager(&app)?
            .current()
            .is_some_and(|state| state.meeting_id == id)
    {
        return Err(CommandError::new(
            CommandErrorCode::Busy,
            "Stop the meeting before deleting it",
        ));
    }
    let conn = open_session_db(&app)?;
    SqliteMeetingRepository::new(&conn)
        .delete(&id)
        .map_err(|e| {
            CommandError::logged(
                CommandErrorCode::Internal,
                "Failed to delete the meeting",
                e,
            )
        })?;
    let app_data = app_data_dir(&app).map_err(|e| {
        CommandError::logged(CommandErrorCode::Internal, "Failed to resolve app data", e)
    })?;
    remove_meeting_audio_dir(&app_data, &id)
}

/// FR-009-02: the user acknowledged the first-use consent modal — persisted
/// so the gate opens from now on.
#[tauri::command]
#[specta::specta]
pub fn meeting_consent_accept(app: AppHandle) -> CommandResult<()> {
    let mut settings = get_settings(&app);
    if !settings.meeting_consent_acknowledged {
        settings.meeting_consent_acknowledged = true;
        write_settings(&app, settings);
    }
    Ok(())
}

/// FR-009-02: the configurable reminder text for "Copiar aviso para o chat".
#[tauri::command]
#[specta::specta]
pub fn meeting_consent_copy(app: AppHandle) -> CommandResult<String> {
    Ok(get_settings(&app).meeting_consent_text)
}

// -- T-067: post-processing -------------------------------------------------

fn post_processor(app: &AppHandle) -> CommandResult<MeetingPostProcessor> {
    app.try_state::<MeetingPostProcessor>()
        .map(|s| s.inner().clone())
        .ok_or_else(|| {
            CommandError::logged(
                CommandErrorCode::Internal,
                "Meeting post-processing is unavailable",
                "MeetingPostProcessor is not managed",
            )
        })
}

/// Load the meeting the T-067 commands act on — 404 for a missing row,
/// `invalid_input` while a session still owns it.
fn load_meeting(app: &AppHandle, meeting_id: &str) -> CommandResult<Meeting> {
    let conn = open_session_db(app)?;
    let meeting = SqliteMeetingRepository::new(&conn)
        .get(meeting_id)
        .map_err(|e| {
            CommandError::logged(CommandErrorCode::Internal, "Failed to load the meeting", e)
        })?
        .ok_or_else(|| CommandError::new(CommandErrorCode::NotFound, "Meeting not found"))?;
    if matches!(meeting.status.as_str(), "recording" | "paused") {
        return Err(CommandError::new(
            CommandErrorCode::InvalidInput,
            "The meeting is still recording",
        ));
    }
    Ok(meeting)
}

/// FR-009-20: re-run the summary + title suggestion only (steps 4–5) —
/// transcript and notes are untouched. A missing summary key answers with
/// `missing_api_key`; other provider failures with `provider`.
#[tauri::command]
#[specta::specta]
pub async fn meeting_regenerate_summary(app: AppHandle, meeting_id: String) -> CommandResult<()> {
    let meeting = load_meeting(&app, &meeting_id)?;
    if meeting.status == "processing" {
        return Err(CommandError::new(
            CommandErrorCode::Busy,
            "The meeting is still processing",
        ));
    }
    let (tx, rx) = oneshot::channel();
    post_processor(&app)?.request(Job::SummaryOnly {
        meeting_id,
        reply: tx,
    })?;
    rx.await.map_err(|_| {
        CommandError::new(
            CommandErrorCode::Internal,
            "Meeting post-processing is unavailable",
        )
    })?
}

/// FR-009-22: re-run the whole pipeline for a meeting that ended in
/// `error` (or `recovered` after a crash). Pending-block coverage makes the
/// re-run idempotent — only audio still missing a segment is retranscribed.
#[tauri::command]
#[specta::specta]
pub async fn meeting_retry_processing(app: AppHandle, meeting_id: String) -> CommandResult<()> {
    load_meeting(&app, &meeting_id)?;
    let (tx, rx) = oneshot::channel();
    post_processor(&app)?.request(Job::Full {
        meeting_id,
        reply: Some(tx),
    })?;
    rx.await.map_err(|_| {
        CommandError::new(
            CommandErrorCode::Internal,
            "Meeting post-processing is unavailable",
        )
    })?
}

/// Every `summary_templates` row for the meeting-window picker (FR-009-17).
#[tauri::command]
#[specta::specta]
pub fn meeting_summary_templates(app: AppHandle) -> CommandResult<Vec<SummaryTemplate>> {
    let conn = open_session_db(&app)?;
    SqliteSummaryTemplateRepository::new(&conn)
        .list()
        .map_err(|e| {
            CommandError::logged(
                CommandErrorCode::Internal,
                "Failed to list summary templates",
                e,
            )
        })
}

/// Select the template a meeting summarizes under (FR-009-17/20):
/// `meeting_id` set → per-meeting override (`meetings.template_id`;
/// `template_id: null` clears it back to the default); `meeting_id: null`
/// → global default (`summary_templates.is_default`). Unknown template ids
/// are rejected.
#[tauri::command]
#[specta::specta]
pub fn meeting_set_summary_template(
    app: AppHandle,
    meeting_id: Option<String>,
    template_id: Option<String>,
) -> CommandResult<()> {
    let conn = open_session_db(&app)?;
    let templates = SqliteSummaryTemplateRepository::new(&conn);
    if let Some(id) = template_id.as_deref() {
        let exists = templates
            .get(id)
            .map_err(|e| {
                CommandError::logged(
                    CommandErrorCode::Internal,
                    "Failed to load the summary template",
                    e,
                )
            })?
            .is_some();
        if !exists {
            return Err(CommandError::new(
                CommandErrorCode::NotFound,
                "Summary template not found",
            ));
        }
    }
    match (meeting_id, template_id) {
        (Some(meeting_id), template_id) => {
            load_meeting(&app, &meeting_id)?;
            SqliteMeetingRepository::new(&conn)
                .set_template_id(&meeting_id, template_id.as_deref())
                .map_err(|e| {
                    CommandError::logged(
                        CommandErrorCode::Internal,
                        "Failed to set the meeting template",
                        e,
                    )
                })
        }
        (None, Some(template_id)) => templates.set_default(&template_id).map_err(|e| {
            CommandError::logged(
                CommandErrorCode::Internal,
                "Failed to set the default summary template",
                e,
            )
        }),
        (None, None) => Err(CommandError::new(
            CommandErrorCode::InvalidInput,
            "A template id is required",
        )),
    }
}
