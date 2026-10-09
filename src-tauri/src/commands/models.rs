use crate::commands::{CommandError, CommandErrorCode, CommandResult};
use crate::managers::hardware::ModelRecommendations;
use crate::managers::model::{ImportedModel, ModelInfo, ModelManager};
use crate::managers::transcription::{ModelStateEvent, TranscriptionManager};
use crate::settings::{get_settings, write_settings, ModelUnloadTimeout};
use crate::stt::selection;
use log::error;
use serde::{Deserialize, Serialize};
use specta::Type;
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
        .map_err(|e| {
            CommandError::logged(CommandErrorCode::Internal, "Model import task failed", e)
        })?
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
    // If deleting the active model, unload it and clear the setting. Any
    // provider selection pointing at this model is cleared too (FR-003-08:
    // "se era o provedor em uso, pedir outro" — the UI then asks for a new
    // pick instead of silently keeping a dead reference).
    let mut settings = get_settings(&app_handle);
    let mut dirty = false;

    if settings.selected_model == model_id {
        transcription_manager
            .unload_model()
            .map_err(|e| CommandError::new(CommandErrorCode::Model, e.to_string()))?;
        settings.selected_model = String::new();
        dirty = true;
    }
    for field in [
        &mut settings.dictation_provider_id,
        &mut settings.meeting_provider_id,
        &mut settings.fallback_provider_id,
    ] {
        if selection::provider_is_local_model(field.as_deref(), &model_id) {
            *field = None;
            dirty = true;
        }
    }
    if dirty {
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
    let _loading_guard = transcription_manager
        .try_start_loading_for(model_id)
        .ok_or_else(|| {
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

/// Which slot a STT provider selection fills (FR-003-03): dictation, meeting,
/// or the optional fallback.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum SttUsage {
    Dictation,
    Meeting,
    Fallback,
}

/// Select the STT provider for a usage slot (FR-003-03).
///
/// In v1 the selectable providers are the installed local models, addressed as
/// `local_model:<model_id>` (see `stt::selection`):
/// - `Dictation` requires a `local_model:` id and is the real engine switch —
///   it delegates to [`switch_active_model`] so the model is loaded and
///   `selected_model` stays the single source of truth.
/// - `Meeting`/`Fallback` accept `null` (meeting inherits dictation; fallback
///   becomes unset) or a `local_model:` id persisted verbatim.
/// - Any other id is a `providers`-table reference and is rejected with
///   `NotFound` until cloud providers land in v1.1+.
#[tauri::command]
#[specta::specta]
pub async fn set_stt_provider(
    app_handle: AppHandle,
    model_manager: State<'_, Arc<ModelManager>>,
    usage: SttUsage,
    provider_id: Option<String>,
) -> CommandResult<()> {
    let model_id = provider_id
        .as_deref()
        .map(|id| {
            selection::local_model_id(id).ok_or_else(|| {
                CommandError::new(
                    CommandErrorCode::NotFound,
                    "Provider not found (v1 only supports local models)",
                )
            })
        })
        .transpose()?;

    if let Some(model_id) = model_id {
        let info = model_manager.get_model_info(model_id).ok_or_else(|| {
            CommandError::new(
                CommandErrorCode::NotFound,
                format!("Model not found: {model_id}"),
            )
        })?;
        if !info.is_downloaded {
            return Err(CommandError::new(
                CommandErrorCode::Model,
                format!("Model not downloaded: {model_id}"),
            ));
        }
    }

    match (usage, model_id) {
        (SttUsage::Dictation, Some(model_id)) => switch_active_model(&app_handle, model_id),
        (SttUsage::Dictation, None) => Err(CommandError::new(
            CommandErrorCode::InvalidInput,
            "Dictation always needs a model — pick one instead of clearing it",
        )),
        (SttUsage::Meeting, model_id) => {
            let mut settings = get_settings(&app_handle);
            settings.meeting_provider_id = model_id.map(selection::local_model_provider_id);
            write_settings(&app_handle, settings);
            Ok(())
        }
        (SttUsage::Fallback, model_id) => {
            let mut settings = get_settings(&app_handle);
            settings.fallback_provider_id = model_id.map(selection::local_model_provider_id);
            write_settings(&app_handle, settings);
            Ok(())
        }
    }
}

/// Effective local model id per usage slot after inheritance is applied
/// (FR-003-03): the `local_model:` ids in the `*_provider_id` settings decoded
/// back to model ids, with meeting falling back to the dictation pick.
#[derive(Serialize, Deserialize, Debug, Clone, Type)]
pub struct EffectiveSttModels {
    pub dictation: Option<String>,
    pub meeting: Option<String>,
    pub fallback: Option<String>,
}

/// Read the effective per-usage model picks in one call — the same resolution
/// the backend applies, so the UI does not have to reimplement inheritance.
#[tauri::command]
#[specta::specta]
pub async fn get_effective_stt_models(app_handle: AppHandle) -> CommandResult<EffectiveSttModels> {
    let settings = get_settings(&app_handle);
    Ok(EffectiveSttModels {
        dictation: selection::effective_dictation_model_id(&settings),
        meeting: selection::effective_meeting_model_id(&settings),
        fallback: selection::effective_fallback_model_id(&settings),
    })
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
    Ok(transcription_manager.is_loading_model())
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
