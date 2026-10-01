//! Toast window commands (F008 toast, T-062).
//!
//! - `toast_set_collapsed` — the webview's 60 s no-interaction timer reports
//!   collapse; the backend hides the window and rebroadcasts `toast://state`
//!   so the Flow Bar's amber dot appears (FR-008-10).
//! - `toast_reopen` — hover on that amber dot reopens the toast.
//! - `toast_dismiss` — ✕, finished confirmation or post-action cleanup.
//! - `toast_set_content_height` — the webview reports its rendered height so
//!   the native window is only ever as tall as the card (no click-through
//!   dead zones like the Flow Bar's).
//!
//! `detector_respond` itself is the T-061 lane's command — the toast frontend
//! calls it directly (`invoke("detector_respond", …)`); nothing here wraps it.

use super::CommandResult;
use tauri::AppHandle;

#[tauri::command]
#[specta::specta]
pub fn toast_set_collapsed(app: AppHandle, collapsed: bool) -> CommandResult<()> {
    crate::toast::set_collapsed(&app, collapsed);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn toast_reopen(app: AppHandle) -> CommandResult<()> {
    crate::toast::reopen(&app);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn toast_dismiss(app: AppHandle) -> CommandResult<()> {
    crate::toast::dismiss(&app);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn toast_set_content_height(app: AppHandle, height: f64) -> CommandResult<()> {
    crate::toast::set_content_height(&app, height);
    Ok(())
}
