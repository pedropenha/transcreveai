//! `detector_respond` — the toast's answer back into the detector
//! (contracts.md §5, FR-008-08, T-061).
//!
//! Actions (`action` arg):
//!
//! | action             | Effect                                                                             |
//! | ------------------ | ---------------------------------------------------------------------------------- |
//! | `start`            | Emit `detector://start-requested {mic_only: false}` — T-064 starts the session.     |
//! | `start_mic_only`   | Same, with `mic_only: true`.                                                        |
//! | `dismiss`          | Mark the detection dismissed; emit `detector://meeting {ended: true, dismissed}` — |
//! |                    | the same meeting never re-fires (AC-008-07).                                        |
//! | `ignore_meeting`   | Alias of `dismiss` ("Ignorar esta reunião").                                        |
//! | `always`           | Set the matching rule's `action = 'auto_start'` (creating one for FR-008-03        |
//! |                    | any-call detections) and emit `detector://start-requested {mic_only: false}`.      |
//! | `never`            | Set the matching rule's `action = 'ignore'` (same create-if-missing) and dismiss.  |
//!
//! Unknown `detection_id` → `NotFound`; unknown `action` → `InvalidInput`.

use super::meeting_rules::open_rules_db;
use super::{CommandError, CommandErrorCode, CommandResult};
use crate::db::meetings::{
    MeetingAppRule, MeetingAppRuleRepository, SqliteMeetingAppRuleRepository,
};
use crate::meeting::{
    Detection, DetectorMeetingEvent, DetectorStartRequest, SharedDetector, DETECTOR_MEETING_EVENT,
    DETECTOR_START_REQUESTED_EVENT,
};
use tauri::{AppHandle, Emitter, State};

fn not_found() -> CommandError {
    CommandError::new(CommandErrorCode::NotFound, "Unknown detection")
}

fn emit_start_request(app: &AppHandle, detection: &Detection, mic_only: bool) -> CommandResult<()> {
    app.emit(
        DETECTOR_START_REQUESTED_EVENT,
        DetectorStartRequest {
            detection_id: detection.detection_id.clone(),
            app_label: detection.app_label.clone(),
            exe: detection.exe_name.clone(),
            exe_path: detection.exe_path.clone(),
            mic_only,
            // User clicked a toast action — 'auto_prompt', not 'auto_start'.
            auto: false,
        },
    )
    .map_err(|e| {
        CommandError::logged(
            CommandErrorCode::Internal,
            "Failed to request the meeting start",
            e,
        )
    })
}

fn emit_dismissal(app: &AppHandle, detection_id: &str) -> CommandResult<()> {
    app.emit(
        DETECTOR_MEETING_EVENT,
        DetectorMeetingEvent::dismissed(detection_id),
    )
    .map_err(|e| {
        CommandError::logged(
            CommandErrorCode::Internal,
            "Failed to emit the detection dismissal",
            e,
        )
    })
}

/// Set `action` on the rule this detection matched — or create one when the
/// detection came from "detectar qualquer chamada" (FR-008-03), where no rule
/// exists for the exe yet and the user's choice must still stick.
fn apply_rule_action(app: &AppHandle, detection: &Detection, action: &str) -> CommandResult<()> {
    let conn = open_rules_db(app)?;
    let repo = SqliteMeetingAppRuleRepository::new(&conn);
    let candidates = repo.find_by_exe(&detection.exe_name).map_err(|e| {
        CommandError::logged(
            CommandErrorCode::Internal,
            "Failed to read the meeting app rules",
            e,
        )
    })?;
    // Several rules can share an exe (Meet on five browsers, Teams exes):
    // prefer the one whose label the detection actually carries.
    let rule = candidates
        .iter()
        .find(|r| r.label == detection.app_label)
        .or_else(|| candidates.first());
    match rule {
        Some(rule) => repo.set_action(&rule.id, action).map_err(|e| {
            CommandError::logged(
                CommandErrorCode::Internal,
                "Failed to update the meeting app rule",
                e,
            )
        }),
        None => {
            let mut rule = MeetingAppRule::new(&detection.exe_name, &detection.app_label, action);
            rule.title_pattern = None;
            repo.create(&rule).map_err(|e| {
                CommandError::logged(
                    CommandErrorCode::Internal,
                    "Failed to save the meeting app rule",
                    e,
                )
            })
        }
    }
}

/// Answer a `detector://meeting` toast. See the module docs for the action
/// table (contracts.md §5).
#[tauri::command]
#[specta::specta]
pub fn detector_respond(
    app: AppHandle,
    detector: State<'_, SharedDetector>,
    detection_id: String,
    action: String,
) -> CommandResult<()> {
    // Fetch what each action needs while holding the lock briefly; the db
    // write + emit happen after it is released.
    let detection = {
        let mut guard = detector.lock().unwrap_or_else(|e| e.into_inner());
        match action.as_str() {
            "dismiss" | "ignore_meeting" | "never" => guard.dismiss(&detection_id),
            _ => guard.find(&detection_id).cloned(),
        }
        .ok_or_else(not_found)?
    };

    match action.as_str() {
        "start" => emit_start_request(&app, &detection, false),
        "start_mic_only" => emit_start_request(&app, &detection, true),
        "dismiss" | "ignore_meeting" => emit_dismissal(&app, &detection_id),
        "always" => {
            apply_rule_action(&app, &detection, "auto_start")?;
            emit_start_request(&app, &detection, false)
        }
        "never" => {
            apply_rule_action(&app, &detection, "ignore")?;
            emit_dismissal(&app, &detection_id)
        }
        _ => Err(CommandError::new(
            CommandErrorCode::InvalidInput,
            "Unknown detector response action",
        )),
    }
}
