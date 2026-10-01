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

use tauri::{AppHandle, Manager};

use crate::db::meetings::{Meeting, MeetingRepository, SqliteMeetingRepository};
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

/// One meeting row (hub detail / meeting window).
#[tauri::command]
#[specta::specta]
pub fn meeting_get(app: AppHandle, id: String) -> CommandResult<Option<Meeting>> {
    let conn = open_session_db(&app)?;
    SqliteMeetingRepository::new(&conn).get(&id).map_err(|e| {
        CommandError::logged(CommandErrorCode::Internal, "Failed to load the meeting", e)
    })
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

/// Delete a meeting row (segments/notes cascade) plus its audio blocks —
/// the dir is only removed when it sits inside `audio/meetings/`.
#[tauri::command]
#[specta::specta]
pub fn meeting_delete(app: AppHandle, id: String) -> CommandResult<()> {
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
