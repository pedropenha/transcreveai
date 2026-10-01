//! Assistant-overlay commands (F012, T-091).
//!
//! Thin IPC layer over [`crate::assistant`] — the session itself owns the
//! conversation state, the provider call and the window. Every command
//! returns the standard `CommandResult` envelope; state changes reach the
//! panel through the `assistant://state` event (and `assistant_get_state`
//! lets a cold webview hydrate on mount).

use super::{CommandError, CommandErrorCode, CommandResult};
use crate::assistant::{self, AssistantStateEvent};
use crate::settings;
use tauri::AppHandle;

/// Full snapshot for the panel — also consumes the `pending_hotkey` flag so
/// a press that arrived while the webview was booting isn't lost.
#[tauri::command]
#[specta::specta]
pub fn assistant_get_state(app: AppHandle) -> CommandResult<AssistantStateEvent> {
    Ok(assistant::state_event(&app, true))
}

/// FR-012-13: send the (edited) prompt. Explicit user action only — dictation
/// alone never submits.
#[tauri::command]
#[specta::specta]
pub fn assistant_send(app: AppHandle, text: String) -> CommandResult<()> {
    assistant::send(&app, &text)
}

/// Re-run the last failed call (the failed user message stays last).
#[tauri::command]
#[specta::specta]
pub fn assistant_retry(app: AppHandle) -> CommandResult<()> {
    assistant::retry(&app)
}

/// Cancel the in-flight call (AC-012-05) — the provider future is aborted;
/// for `cli_agent/*` the subprocess dies with it.
#[tauri::command]
#[specta::specta]
pub fn assistant_cancel(app: AppHandle) -> CommandResult<()> {
    assistant::cancel_in_flight(&app);
    Ok(())
}

/// Esc from the panel: cancel while thinking, close otherwise.
#[tauri::command]
#[specta::specta]
pub fn assistant_dismiss(app: AppHandle) -> CommandResult<()> {
    assistant::dismiss(&app);
    Ok(())
}

/// × button — hides the panel; the session is kept in memory.
#[tauri::command]
#[specta::specta]
pub fn assistant_close(app: AppHandle) -> CommandResult<()> {
    assistant::close_panel(&app);
    Ok(())
}

/// "Nova conversa" (FR-012-15) — aborts any in-flight call and clears the
/// in-memory history.
#[tauri::command]
#[specta::specta]
pub fn assistant_new_conversation(app: AppHandle) -> CommandResult<()> {
    assistant::new_conversation(&app);
    Ok(())
}

/// The panel was clicked — take keyboard focus on the OS side
/// (FR-012-11: the panel only ever focuses on explicit intent).
#[tauri::command]
#[specta::specta]
pub fn assistant_focus(app: AppHandle) -> CommandResult<()> {
    assistant::focus_panel(&app);
    Ok(())
}

/// FR-012-04: persist the assistant provider choice (`None` resets to auto).
/// Accepts any known `post_process_providers` id, including `cli_agent/*`.
#[tauri::command]
#[specta::specta]
pub fn set_assistant_provider(app: AppHandle, provider_id: Option<String>) -> CommandResult<()> {
    let mut settings = settings::get_settings(&app);
    match provider_id.as_deref() {
        None | Some("") | Some("auto") => {
            settings.assistant_provider_id = None;
        }
        Some(id) => {
            if settings.post_process_provider(id).is_none() {
                return Err(CommandError::new(
                    CommandErrorCode::NotFound,
                    format!("Provider '{id}' not found"),
                ));
            }
            settings.assistant_provider_id = Some(id.to_string());
        }
    }
    settings::write_settings(&app, settings);
    // The panel may be open — refresh its provider chip/empty state.
    assistant::emit_state(&app);
    Ok(())
}
