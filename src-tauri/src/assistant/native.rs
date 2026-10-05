//! Checked OS operations and respect for Windows transparency preferences.
use crate::commands::{CommandError, CommandErrorCode, CommandResult};
use tauri::AppHandle;
#[cfg(target_os = "windows")]
pub(super) mod windows;
/// Await the OS operation rather than reporting a successful queued request.
pub(super) async fn run_native_checked(
    app: &AppHandle,
    operation: fn(&AppHandle) -> CommandResult<()>,
) -> CommandResult<()> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let handle = app.clone();
    app.run_on_main_thread(move || {
        let _ = sender.send(operation(&handle));
    })
    .map_err(|error| {
        CommandError::logged(
            CommandErrorCode::Internal,
            "Failed to schedule assistant operation",
            error,
        )
    })?;
    let received = tokio::time::timeout(std::time::Duration::from_secs(3), receiver)
        .await
        .map_err(|_| {
            CommandError::new(CommandErrorCode::Internal, "Assistant operation timed out")
        })?;
    received.map_err(|error| {
        CommandError::logged(
            CommandErrorCode::Internal,
            "Assistant operation was interrupted",
            error,
        )
    })?
}

pub(crate) fn transparency_allowed(value: Option<u32>) -> bool {
    value != Some(0)
}

#[cfg(target_os = "windows")]
pub(super) fn windows_transparency_enabled() -> bool {
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};
    let value = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
        .ok()
        .and_then(|key| key.get_value::<u32, _>("EnableTransparency").ok());
    transparency_allowed(value)
}

#[cfg(not(target_os = "macos"))]
use tauri::Manager;
#[cfg(not(target_os = "macos"))]
pub(super) fn hide_native(app: &AppHandle) -> CommandResult<()> {
    let window = app
        .get_webview_window(crate::window_labels::ASSISTANT)
        .ok_or_else(|| {
            CommandError::new(
                CommandErrorCode::NotFound,
                "Assistant window is unavailable",
            )
        })?;
    #[cfg(target_os = "windows")]
    {
        let hwnd = window.hwnd().map_err(|error| {
            CommandError::logged(
                CommandErrorCode::Internal,
                "Assistant HWND unavailable",
                error,
            )
        })?;
        windows::hide(hwnd).map_err(|error| {
            CommandError::logged(
                CommandErrorCode::Internal,
                "Failed to hide assistant",
                error,
            )
        })
    }
    #[cfg(not(target_os = "windows"))]
    window.hide().map_err(|error| {
        CommandError::logged(
            CommandErrorCode::Internal,
            "Failed to hide assistant",
            error,
        )
    })
}
