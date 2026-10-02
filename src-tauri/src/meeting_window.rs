//! The meeting window (F009, T-066; FR-009-12..14).
//!
//! A normal *decorated* singleton window (`window_labels::MEETING`) hosting
//! "Minhas notas" / Transcrição / Resumo. Unlike the Flow Bar and the toast it
//! is created lazily — `meeting_window_open` is a user gesture, so the WebView2
//! cold-start cost is acceptable and there is no reason to keep a webview
//! resident for it.
//!
//! Behavior decisions:
//! - FR-009-14: closing the window never stops the session. The global
//!   `on_window_event` in `lib.rs` already turns CloseRequested into `hide()`
//!   for every non-hub window, so this window inherits it for free — reopening
//!   re-shows the same webview with all its state.
//! - The window shows one meeting at a time. `open` resolves the target id
//!   (explicit arg → the active session → the most recent row), passes it as
//!   `?meeting_id=` on first creation and re-broadcasts `meeting://open` on
//!   every call — the webview also re-reads `meeting_current` on mount, so a
//!   listener that attaches late still converges on the right meeting.
//! - The meeting window is never auto-opened: starts coming from the
//!   detection toast or auto-start rules already confirm via toast, and the
//!   Flow Bar pill is the persistent indicator (FR-009-07). Only manual
//!   starts (Hub "Nova reunião") open the window.

use std::sync::Mutex;

use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::commands::{CommandError, CommandErrorCode, CommandResult};
use crate::db::meetings::{MeetingRepository, SqliteMeetingRepository};
use crate::meeting::session::{open_session_db, MeetingSessionManager};
use crate::window_labels::MEETING;

/// Emitted to the meeting window whenever `meeting_window_open` (re)targets
/// it — the webview switches the displayed meeting (`{ meeting_id }`).
pub const MEETING_OPEN_EVENT: &str = "meeting://open";

/// Meeting the singleton window is currently showing — used by
/// `meeting_rename` to keep the native title bar in sync.
static SHOWN_MEETING: Mutex<Option<String>> = Mutex::new(None);

fn lock_shown() -> std::sync::MutexGuard<'static, Option<String>> {
    SHOWN_MEETING.lock().unwrap_or_else(|e| e.into_inner())
}

/// `meeting://open` payload.
#[derive(Clone, Debug, serde::Serialize)]
pub struct MeetingOpenPayload {
    pub meeting_id: String,
}

/// Initial URL for a fresh window — `?meeting_id=` is read by the webview on
/// mount (before any event listener could attach).
fn meeting_url(meeting_id: &str) -> String {
    format!("src/meeting/index.html?meeting_id={meeting_id}")
}

/// Native title bar text for a meeting.
fn window_title(meeting_title: &str) -> String {
    let trimmed = meeting_title.trim();
    if trimmed.is_empty() {
        "Transcreve.ai".to_string()
    } else {
        format!("{trimmed} — Transcreve.ai")
    }
}

/// Which meeting `open` should show: the explicit argument wins, then the
/// live session (recording/paused), then the most recent row.
fn pick_meeting_id(
    requested: Option<&str>,
    active: Option<&str>,
    latest: Option<&str>,
) -> Option<String> {
    if let Some(id) = requested.map(str::trim).filter(|id| !id.is_empty()) {
        return Some(id.to_string());
    }
    if let Some(id) = active {
        return Some(id.to_string());
    }
    latest.map(str::to_string)
}

fn resolve_meeting_id(app: &AppHandle, requested: Option<String>) -> CommandResult<String> {
    let active = app
        .try_state::<MeetingSessionManager>()
        .and_then(|m| m.current())
        .filter(|state| state.status == "recording" || state.status == "paused")
        .map(|state| state.meeting_id);
    let conn = open_session_db(app)?;
    let latest = SqliteMeetingRepository::new(&conn)
        .list()
        .map_err(|e| {
            CommandError::logged(CommandErrorCode::Internal, "Failed to list meetings", e)
        })?
        .into_iter()
        .next()
        .map(|m| m.id);
    pick_meeting_id(requested.as_deref(), active.as_deref(), latest.as_deref())
        .ok_or_else(|| CommandError::new(CommandErrorCode::NotFound, "There is no meeting to show"))
}

fn meeting_title(app: &AppHandle, meeting_id: &str) -> CommandResult<String> {
    let conn = open_session_db(app)?;
    SqliteMeetingRepository::new(&conn)
        .get(meeting_id)
        .map_err(|e| {
            CommandError::logged(CommandErrorCode::Internal, "Failed to load the meeting", e)
        })?
        .map(|m| m.title)
        .ok_or_else(|| CommandError::new(CommandErrorCode::NotFound, "Meeting not found"))
}

/// `meeting_window_open` — create-or-focus the meeting window on `meeting_id`.
///
/// Never blocks the caller: the id + title are resolved on the calling thread
/// (cheap SQLite reads) and the window work runs on a dedicated thread. This
/// MUST NOT go through `run_on_main_thread`: on Windows, `WebviewWindowBuilder
/// ::build()` inside an event-loop handler leaves the WebView2 controller
/// waiting on a nested message pump that never completes — the window ends up
/// stuck on `about:blank` with no IPC (dead window). Called from a worker
/// thread instead, `build()` dispatches `CreateWindow` through the event-loop
/// proxy and the webview initializes normally. This also keeps the tray path
/// (already on the main thread) working.
pub fn open(app: &AppHandle, meeting_id: Option<String>) -> CommandResult<()> {
    let id = resolve_meeting_id(app, meeting_id)?;
    let title = meeting_title(app, &id)?;
    let handle = app.clone();
    std::thread::Builder::new()
        .name("meeting-window-open".into())
        .spawn(move || {
            if let Err(e) = open_on_main(&handle, &id, &title) {
                log::error!("Failed to open the meeting window: {}", e.message);
                emit_error_toast(&handle);
            }
        })
        .map_err(|e| {
            CommandError::logged(
                CommandErrorCode::Internal,
                "Failed to open the meeting window",
                e,
            )
        })?;
    Ok(())
}

/// Update the native title bar after `meeting_rename` — only when the window
/// is showing that meeting.
pub fn retitle_if_shown(app: &AppHandle, meeting_id: &str, meeting_title: &str) {
    if lock_shown().as_deref() != Some(meeting_id) {
        return;
    }
    let title = window_title(meeting_title);
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(window) = handle.get_webview_window(MEETING) {
            if let Err(e) = window.set_title(&title) {
                log::warn!("Failed to retitle the meeting window: {e}");
            }
        }
    });
}

fn emit_error_toast(app: &AppHandle) {
    use crate::meeting::session::{ToastPayload, TOAST_SHOW_EVENT};
    let lang = crate::settings::get_settings(app).app_language;
    let message = if lang == "pt-BR" {
        "Não foi possível abrir a janela da reunião."
    } else {
        "Could not open the meeting window."
    };
    let _ = app.emit(
        TOAST_SHOW_EVENT,
        ToastPayload {
            kind: "meeting_error".to_string(),
            message: message.to_string(),
            action: None,
            meeting_id: None,
        },
    );
}

/// Worker-thread side of [`open`]: focus the existing window or build it.
/// `WebviewWindow` methods dispatch through the event-loop proxy, so calling
/// them off the main thread is fine — and on Windows it is required (see
/// [`open`]).
fn open_on_main(app: &AppHandle, meeting_id: &str, meeting_title: &str) -> CommandResult<()> {
    let title = window_title(meeting_title);
    if let Some(window) = app.get_webview_window(MEETING) {
        if let Err(e) = window.unminimize() {
            log::warn!("Failed to unminimize the meeting window: {e}");
        }
        if let Err(e) = window.show() {
            log::warn!("Failed to show the meeting window: {e}");
        }
        if let Err(e) = window.set_title(&title) {
            log::warn!("Failed to retitle the meeting window: {e}");
        }
        if let Err(e) = window.set_focus() {
            log::warn!("Failed to focus the meeting window: {e}");
        }
    } else {
        let mut builder = WebviewWindowBuilder::new(
            app,
            MEETING,
            WebviewUrl::App(meeting_url(meeting_id).into()),
        )
        .title(&title)
        .inner_size(760.0, 560.0)
        .min_inner_size(480.0, 360.0)
        .resizable(true)
        .visible(false);

        if let Some(data_dir) = crate::portable::data_dir() {
            builder = builder.data_directory(data_dir.join("webview"));
        }

        let window = builder.build().map_err(|e| {
            CommandError::logged(
                CommandErrorCode::Internal,
                "Failed to create the meeting window",
                e,
            )
        })?;

        // Same rationale as the Hub (cjpais/Handy#1940): WebView2 browser
        // accelerators (F5/F6/…) have no place in an app window and F5 would
        // reload mid-note-taking.
        #[cfg(target_os = "windows")]
        crate::utils::disable_webview2_accelerators(&window);

        if let Err(e) = window.show() {
            log::warn!("Failed to show the meeting window: {e}");
        }
        if let Err(e) = window.set_focus() {
            log::warn!("Failed to focus the meeting window: {e}");
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Err(e) = app.set_activation_policy(tauri::ActivationPolicy::Regular) {
            log::error!("Failed to set activation policy to Regular: {}", e);
        }
    }

    *lock_shown() = Some(meeting_id.to_string());
    // Redundant with `?meeting_id=` on first load; essential when the window
    // is reused for another meeting.
    if let Err(e) = app.emit_to(
        MEETING,
        MEETING_OPEN_EVENT,
        MeetingOpenPayload {
            meeting_id: meeting_id.to_string(),
        },
    ) {
        log::warn!("Failed to emit {MEETING_OPEN_EVENT}: {e}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_carries_the_meeting_id() {
        assert_eq!(
            meeting_url("abc-123"),
            "src/meeting/index.html?meeting_id=abc-123"
        );
    }

    #[test]
    fn title_composes_with_app_name_and_falls_back() {
        assert_eq!(
            window_title("Zoom · 05/02 14:30"),
            "Zoom · 05/02 14:30 — Transcreve.ai"
        );
        assert_eq!(window_title("   "), "Transcreve.ai");
    }

    #[test]
    fn resolution_prefers_requested_then_active_then_latest() {
        assert_eq!(
            pick_meeting_id(Some("m-1"), Some("m-2"), Some("m-3")).as_deref(),
            Some("m-1")
        );
        // Blank requests fall through to the active session.
        assert_eq!(
            pick_meeting_id(Some("  "), Some("m-2"), Some("m-3")).as_deref(),
            Some("m-2")
        );
        assert_eq!(
            pick_meeting_id(None, Some("m-2"), Some("m-3")).as_deref(),
            Some("m-2")
        );
        assert_eq!(
            pick_meeting_id(None, None, Some("m-3")).as_deref(),
            Some("m-3")
        );
        assert_eq!(pick_meeting_id(None, None, None), None);
    }
}
