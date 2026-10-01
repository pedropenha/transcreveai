use crate::commands::{CommandError, CommandErrorCode, CommandResult};
use crate::managers::hardware::ModelRecommendations;
use crate::managers::model::{ImportedModel, ModelInfo, ModelManager};
use crate::managers::transcription::{ModelStateEvent, TranscriptionManager};
use crate::settings::{get_settings, write_settings, ModelUnloadTimeout};
use log::error;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};

#[tauri::command]
#[specta::specta]
pub async fn get_available_models(
    model_manager: State<'_, Arc<ModelManager>>,
) -> CommandResult<Vec<ModelInfo>> {
    Ok(model_manager.get_available_models())
}

#[tauri::command]
#[specta::specta]
pub async fn get_model_info(
    model_manager: State<'_, Arc<ModelManager>>,
    model_id: String,
) -> CommandResult<Option<ModelInfo>> {
    Ok(model_manager.get_model_info(&model_id))
}

/// Re-scan local sources (custom models dir + shared HF cache) for models added
/// since launch
#[tauri::command]
#[specta::specta]
pub async fn rescan_local_models(model_manager: State<'_, Arc<ModelManager>>) -> CommandResult<()> {
    let mm = model_manager.inner().clone();
    tokio::task::spawn_blocking(move || mm.rescan_local_models())
        .await
        .map_err(|e| {
            CommandError::logged(CommandErrorCode::Internal, "Model rescan task failed", e)
        })?
        .map_err(|e| CommandError::new(CommandErrorCode::Model, e.to_string()))
}

/// Import a local `.gguf`/`.bin` model file into the models directory
/// (FR-003-07): the file is format-checked, disk space is verified before the
/// copy, and the SHA-256 comes back for display.
#[tauri::command]
#[specta::specta]
pub async fn import_model(
    model_manager: State<'_, Arc<ModelManager>>,
    path: String,
) -> CommandResult<ImportedModel> {
    let mm = model_manager.inner().clone();
    tokio::task::spawn_blocking(move || mm.import_custom_model(std::path::Path::new(&path)))
        .await
        .map_err(|e| CommandError::logged(CommandErrorCode::Internal, "Model import task failed", e))?
        .map_err(|e| CommandError::new(CommandErrorCode::Model, e.to_string()))
}

/// Hardware probe + per-model suitability labels (FR-003-05). GPU enumeration
/// goes through the transcription engine's device list, so first call may load
/// backend libraries — hence the blocking pool.
#[tauri::command]
#[specta::specta]
pub async fn get_model_recommendations(
    model_manager: State<'_, Arc<ModelManager>>,
) -> CommandResult<ModelRecommendations> {
    let mm = model_manager.inner().clone();
    tokio::task::spawn_blocking(move || Ok(mm.model_recommendations()))
        .await
        .map_err(|e| {
            CommandError::logged(CommandErrorCode::Internal, "Hardware probe task failed", e)
        })?
}

/// SHA-256 of a downloaded model's file, computed on demand (FR-003-07 shows
/// it for imported models; usable for any file-based model).
#[tauri::command]
#[specta::specta]
pub async fn get_model_sha256(
    model_manager: State<'_, Arc<ModelManager>>,
    model_id: String,
) -> CommandResult<String> {
    let mm = model_manager.inner().clone();
    tokio::task::spawn_blocking(move || mm.model_file_sha256(&model_id))
        .await
        .map_err(|e| CommandError::logged(CommandErrorCode::Internal, "Model hash task failed", e))?
        .map_err(|e| CommandError::new(CommandErrorCode::Model, e.to_string()))
}

#[tauri::command]
#[specta::specta]
pub async fn download_model(
    app_handle: AppHandle,
    model_manager: State<'_, Arc<ModelManager>>,
    model_id: String,
) -> CommandResult<()> {
    let result = model_manager
        .download_model(&model_id)
        .await
        .map_err(|e| CommandError::new(CommandErrorCode::Model, e.to_string()));

    if let Err(ref error) = result {
        // Log as well as emit: the toast is transient, and failed downloads have
        // historically been undiagnosable because logs showed nothing (#1579).
        error!("Model download failed for {}: {}", model_id, error);
        let _ = app_handle.emit(
            "model-download-failed",
            serde_json::json!({ "model_id": &model_id, "error": &error.message }),
        );
    }

    result
}

#[tauri::command]
#[specta::specta]
pub async fn delete_model(
    app_handle: AppHandle,
    model_manager: State<'_, Arc<ModelManager>>,
    transcription_manager: State<'_, Arc<TranscriptionManager>>,
    model_id: String,
) -> CommandResult<()> {
    // If deleting the active model, unload it and clear the setting
    let settings = get_settings(&app_handle);
    if settings.selected_model == model_id {
        transcription_manager
            .unload_model()
            .map_err(|e| CommandError::new(CommandErrorCode::Model, e.to_string()))?;

        let mut settings = get_settings(&app_handle);
        settings.selected_model = String::new();
        write_settings(&app_handle, settings);
    }

    model_manager
        .delete_model(&model_id)
        .map_err(|e| CommandError::new(CommandErrorCode::Model, e.to_string()))
}

/// Shared logic for switching the active model, used by both the Tauri command
/// and the tray menu handler.
///
/// Validates the model, updates the persisted setting, and loads the model
/// unless the unload timeout is set to "Immediately" (in which case the model
/// will be loaded on-demand during the next transcription).
pub fn switch_active_model(app: &AppHandle, model_id: &str) -> CommandResult<()> {
    let model_manager = app.state::<Arc<ModelManager>>();
    let transcription_manager = app.state::<Arc<TranscriptionManager>>();

    // Atomically claim the loading slot — prevents concurrent model loads
    // from tray double-clicks or overlapping commands. The guard resets the
    // flag on drop (including early returns, errors, and panics).
    let _loading_guard = transcription_manager.try_start_loading().ok_or_else(|| {
        CommandError::new(CommandErrorCode::Busy, "Model load already in progress")
    })?;

    // Check if model exists and is available
    let model_info = model_manager.get_model_info(model_id).ok_or_else(|| {
        CommandError::new(
            CommandErrorCode::NotFound,
            format!("Model not found: {model_id}"),
        )
    })?;

    if !model_info.is_downloaded {
        return Err(CommandError::new(
            CommandErrorCode::Model,
            format!("Model not downloaded: {model_id}"),
        ));
    }

    let settings = get_settings(app);
    let unload_timeout = settings.model_unload_timeout;
    let old_model = settings.selected_model.clone();
    let old_onboarding_completed = settings.onboarding_completed;

    // Persist the new selection early so the frontend sees the correct model
    // when it reacts to events emitted by load_model.
    let mut settings = settings;
    settings.selected_model = model_id.to_string();
    settings.onboarding_completed = true;

    write_settings(app, settings);

    // Skip eager loading if unload is set to "Immediately" — the model
    // will be loaded on-demand during the next transcription.
    if unload_timeout == ModelUnloadTimeout::Immediately {
        // Notify frontend — load_model won't be called so no events
        // would otherwise be emitted.
        let _ = app.emit(
            "model-state-changed",
            ModelStateEvent {
                event_type: "selection_changed".to_string(),
                model_id: Some(model_id.to_string()),
                model_name: Some(model_info.name.clone()),
                error: None,
            },
        );
        log::info!(
            "Model selection changed to {} (not loading — unload set to Immediately).",
            model_id
        );
        return Ok(());
    }

    // Load the model. On failure, revert the persisted selection.
    if let Err(e) = transcription_manager.load_model(model_id) {
        let mut settings = get_settings(app);
        settings.selected_model = old_model;
        settings.onboarding_completed = old_onboarding_completed;
        write_settings(app, settings);
        return Err(CommandError::new(CommandErrorCode::Model, e.to_string()));
    }

    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn set_active_model(
    app_handle: AppHandle,
    _model_manager: State<'_, Arc<ModelManager>>,
    _transcription_manager: State<'_, Arc<TranscriptionManager>>,
    model_id: String,
) -> CommandResult<()> {
    switch_active_model(&app_handle, &model_id)
}

#[tauri::command]
#[specta::specta]
pub async fn get_current_model(app_handle: AppHandle) -> CommandResult<String> {
    let settings = get_settings(&app_handle);
    Ok(settings.selected_model)
}

#[tauri::command]
#[specta::specta]
pub async fn get_transcription_model_status(
    transcription_manager: State<'_, Arc<TranscriptionManager>>,
) -> CommandResult<Option<String>> {
    Ok(transcription_manager.get_current_model())
}

#[tauri::command]
#[specta::specta]
pub async fn is_model_loading(
    transcription_manager: State<'_, Arc<TranscriptionManager>>,
) -> CommandResult<bool> {
    // Check if transcription manager has a loaded model
    let current_model = transcription_manager.get_current_model();
    Ok(current_model.is_none())
}

#[tauri::command]
#[specta::specta]
pub async fn cancel_download(
    model_manager: State<'_, Arc<ModelManager>>,
    model_id: String,
) -> CommandResult<()> {
    model_manager
        .cancel_download(&model_id)
        .map_err(|e| CommandError::new(CommandErrorCode::Model, e.to_string()))
}
