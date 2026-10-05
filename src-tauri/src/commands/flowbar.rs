//! Flow Bar commands (F001, T-040).
//!
//! - `flowbar_set_hover` — contracts.md §5: the webview reports the currently
//!   interactive rectangle so the core can keep the rest of the transparent
//!   window click-through (NFR-001-02). The contract calls the arg
//!   `hovering`; a bare bool cannot work — while click-through is on the
//!   webview never sees the cursor — so the bounds form is what is wired.
//! - `flowbar_toggle_dictation` — FR-001-03/06: the 🎤/■ button presses the
//!   same edge as the configured transcribe shortcut (source "FlowBar"), so a
//!   click starts a hands-free session and the ■ press ends-and-inserts it.
//! - `flowbar_start_notetaker` — FR-001-04: the ◉ button. The meeting session
//!   layer is T-064's (meeting/mod.rs says so explicitly); until it lands this
//!   is the documented seam: it logs the request and emits
//!   `notetaker://start-requested`, which T-064 subscribes to — the same
//!   pattern the tray uses for its disabled `toggle_meeting` entry.
//! - `flowbar_retry_last_failed` — AC-001-08 "Tentar novamente": re-runs
//!   transcription of the newest `failed` dictation row's preserved WAV via
//!   the same path as the history screen's per-entry retry.

use super::{CommandError, CommandErrorCode, CommandResult};
use crate::managers::history::HistoryManager;
use crate::managers::transcription::TranscriptionManager;
use crate::overlay::FlowbarRect;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};

/// Event emitted when the ◉ button is pressed and T-064's meeting session
/// layer is not yet wired to consume it.
pub const NOTETAKER_START_REQUESTED_EVENT: &str = "notetaker://start-requested";

#[tauri::command]
#[specta::specta]
pub fn flowbar_set_hover(rect: Option<FlowbarRect>) -> CommandResult<()> {
    crate::overlay::set_interactive_rect(rect);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn flowbar_toggle_dictation(app: AppHandle) -> CommandResult<()> {
    // Same edge as the "transcribe" global shortcut and the
    // --toggle-transcription CLI flag: the coordinator's state machine turns
    // an idle press into a hands-free start and a press during recording into
    // stop-and-insert.
    let session = app
        .try_state::<crate::TranscriptionCoordinator>()
        .and_then(|coordinator| coordinator.current_session());
    let binding =
        flowbar_dictation_binding(session.as_ref().map(|session| session.binding_id.as_str()));
    crate::signal_handle::send_transcription_input(&app, binding, "FlowBar");
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn flowbar_start_notetaker(app: AppHandle) -> CommandResult<()> {
    log::info!("Notetaker start requested from the Flow Bar (session layer lands with T-064)");
    app.emit(NOTETAKER_START_REQUESTED_EVENT, ()).map_err(|e| {
        CommandError::logged(
            CommandErrorCode::Internal,
            "Failed to emit notetaker start request",
            e,
        )
    })
}

#[tauri::command]
#[specta::specta]
pub async fn flowbar_retry_last_failed(
    app: AppHandle,
    history_manager: State<'_, Arc<HistoryManager>>,
    transcription_manager: State<'_, Arc<TranscriptionManager>>,
) -> CommandResult<()> {
    let entry = history_manager.get_latest_failed_entry().map_err(|e| {
        CommandError::logged(
            CommandErrorCode::Internal,
            "Failed to load last failed entry",
            e,
        )
    })?;
    let Some(entry) = entry else {
        return Err(CommandError::new(
            CommandErrorCode::NotFound,
            "No failed dictation to retry",
        ));
    };
    // file_name is empty once the retention cleanup has already dropped the
    // recording; there is nothing left to re-transcribe.
    if entry.file_name.is_empty() {
        return Err(CommandError::new(
            CommandErrorCode::InvalidInput,
            "The failed recording is no longer on disk",
        ));
    }
    super::history::retry_entry_transcription(
        &app,
        &history_manager,
        &transcription_manager,
        entry.id,
    )
    .await
}

/// The stop button must use the action that owns the active translated capture.
pub(crate) fn flowbar_dictation_binding(active: Option<&str>) -> &'static str {
    if active == Some("transcribe_translate") {
        "transcribe_translate"
    } else {
        "transcribe"
    }
}

#[cfg(test)]
mod translation_tests {
    #[test]
    fn translation_flowbar_routes_only_active_translated_capture() {
        assert_eq!(
            super::flowbar_dictation_binding(Some("transcribe_translate")),
            "transcribe_translate"
        );
        assert_eq!(super::flowbar_dictation_binding(None), "transcribe");
        assert_eq!(
            super::flowbar_dictation_binding(Some("assistant")),
            "transcribe"
        );
    }
}
