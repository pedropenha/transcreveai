//! Pairing and configuration are available only to the trusted Hub window.
use crate::commands::{CommandError, CommandErrorCode, CommandResult};
use crate::connector_mcp::{self, McpConfiguration, McpGrant, McpStatus};
use tauri::{AppHandle, WebviewWindow};

fn hub(window: &WebviewWindow) -> CommandResult<()> {
    if window.label() == crate::window_labels::HUB {
        Ok(())
    } else {
        Err(CommandError::new(
            CommandErrorCode::PermissionDenied,
            "connector_mcp.hub_required",
        ))
    }
}

fn error(_: String) -> CommandError {
    CommandError::new(
        CommandErrorCode::PermissionDenied,
        "connector_mcp.unavailable",
    )
}

#[tauri::command]
#[specta::specta]
pub async fn mcp_status(app: AppHandle, window: WebviewWindow) -> CommandResult<McpStatus> {
    hub(&window)?;
    connector_mcp::status(&app).await.map_err(error)
}

#[tauri::command]
#[specta::specta]
pub async fn mcp_pair(
    app: AppHandle,
    window: WebviewWindow,
    label: String,
    connection_ids: Vec<String>,
) -> CommandResult<McpGrant> {
    hub(&window)?;
    connector_mcp::pair(&app, label, connection_ids)
        .await
        .map_err(error)
}

#[tauri::command]
#[specta::specta]
pub async fn mcp_revoke(
    app: AppHandle,
    window: WebviewWindow,
    client_id: String,
) -> CommandResult<()> {
    hub(&window)?;
    connector_mcp::revoke(&app, &client_id).await.map_err(error)
}

#[tauri::command]
#[specta::specta]
pub async fn mcp_config(
    app: AppHandle,
    window: WebviewWindow,
    client_id: String,
) -> CommandResult<McpConfiguration> {
    hub(&window)?;
    connector_mcp::configuration(&app, &client_id)
        .await
        .map_err(error)
}
