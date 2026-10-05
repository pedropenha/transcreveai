//! Secret-free connector IPC. Only the trusted Hub may change connections.
use crate::connectors::{self, types::*};
use tauri::{AppHandle, WebviewWindow};
fn hub(window: &WebviewWindow) -> ConnectorResult<()> {
    if window.label() != crate::window_labels::HUB {
        return Err(ConnectorError::new(ConnectorErrorCode::PolicyDenied));
    }
    Ok(())
}
#[tauri::command]
#[specta::specta]
pub fn connector_list_connections(
    app: AppHandle,
    window: WebviewWindow,
) -> ConnectorResult<Vec<ConnectorConfig>> {
    hub(&window)?;
    connectors::list(&app)
}
#[tauri::command]
#[specta::specta]
pub async fn connector_save_connection(
    app: AppHandle,
    window: WebviewWindow,
    input: ConnectorInput,
    id: Option<String>,
) -> ConnectorResult<ConnectorConfig> {
    hub(&window)?;
    connectors::save(&app, input, id.as_deref()).await
}
#[tauri::command]
#[specta::specta]
pub async fn connector_begin_oauth(
    app: AppHandle,
    window: WebviewWindow,
    connection_id: String,
) -> ConnectorResult<ConnectorConfig> {
    hub(&window)?;
    connectors::begin_oauth(&app, &connection_id).await
}
#[tauri::command]
#[specta::specta]
pub async fn connector_cancel_oauth(
    app: AppHandle,
    window: WebviewWindow,
    connection_id: String,
) -> ConnectorResult<()> {
    hub(&window)?;
    connectors::cancel_oauth(&app, &connection_id).await
}
#[tauri::command]
#[specta::specta]
pub async fn connector_disconnect(
    app: AppHandle,
    window: WebviewWindow,
    connection_id: String,
) -> ConnectorResult<()> {
    hub(&window)?;
    connectors::disconnect(&app, &connection_id).await
}
#[tauri::command]
#[specta::specta]
pub async fn connector_delete_connection(
    app: AppHandle,
    window: WebviewWindow,
    connection_id: String,
) -> ConnectorResult<()> {
    hub(&window)?;
    connectors::delete(&app, &connection_id).await
}
#[tauri::command]
#[specta::specta]
pub async fn connector_test_connection(
    app: AppHandle,
    window: WebviewWindow,
    connection_id: String,
) -> ConnectorResult<ConnectionTest> {
    hub(&window)?;
    connectors::test(&app, &connection_id).await
}
#[tauri::command]
#[specta::specta]
pub async fn connector_azure_catalog(
    app: AppHandle,
    window: WebviewWindow,
    connection_id: String,
    project_id: Option<String>,
    team_id: Option<String>,
) -> ConnectorResult<AzureCatalog> {
    hub(&window)?;
    connectors::catalog(
        &app,
        &connection_id,
        project_id.as_deref(),
        team_id.as_deref(),
    )
    .await
}
#[tauri::command]
#[specta::specta]
pub async fn connector_save_defaults(
    app: AppHandle,
    window: WebviewWindow,
    connection_id: String,
    defaults: AzureDestinationDefaults,
) -> ConnectorResult<ConnectorConfig> {
    hub(&window)?;
    connectors::save_defaults(&app, &connection_id, defaults).await
}
#[tauri::command]
#[specta::specta]
pub async fn connector_read(
    app: AppHandle,
    window: WebviewWindow,
    connection_id: String,
    resource_id: String,
    project_id: Option<String>,
) -> ConnectorResult<RemoteDocument> {
    hub(&window)?;
    connectors::read(&app, &connection_id, &resource_id, project_id.as_deref()).await
}
#[tauri::command]
#[specta::specta]
pub async fn connector_query(
    app: AppHandle,
    window: WebviewWindow,
    connection_id: String,
    project_id: Option<String>,
) -> ConnectorResult<Vec<RemoteDocument>> {
    hub(&window)?;
    connectors::query(&app, &connection_id, project_id.as_deref()).await
}
