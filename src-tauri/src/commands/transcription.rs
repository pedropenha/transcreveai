use crate::commands::{CommandError, CommandErrorCode, CommandResult};
use crate::managers::transcription::TranscriptionManager;
use crate::settings::{get_settings, write_settings, ModelUnloadTimeout};
use serde::Serialize;
use specta::Type;
use tauri::{AppHandle, Manager, State};

#[derive(Serialize, Type)]
pub struct ModelLoadStatus {
    is_loaded: bool,
    current_model: Option<String>,
}

#[tauri::command]
#[specta::specta]
pub fn set_model_unload_timeout(app: AppHandle, timeout: ModelUnloadTimeout) -> CommandResult<()> {
    let mut settings = get_settings(&app);
    settings.model_unload_timeout = timeout;
    write_settings(&app, settings);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn get_model_load_status(
    transcription_manager: State<TranscriptionManager>,
) -> CommandResult<ModelLoadStatus> {
    Ok(ModelLoadStatus {
        is_loaded: transcription_manager.is_model_loaded(),
        current_model: transcription_manager.get_current_model(),
    })
}

#[tauri::command]
#[specta::specta]
pub fn unload_model_manually(
    transcription_manager: State<TranscriptionManager>,
) -> CommandResult<()> {
    transcription_manager
        .unload_model()
        .map_err(|e| CommandError::new(CommandErrorCode::Model, e.to_string()))
}

/// Select a local translation model without changing ordinary dictation.
#[tauri::command]
#[specta::specta]
pub fn change_translation_model_setting(
    app: AppHandle,
    model_id: Option<String>,
) -> CommandResult<()> {
    if let Some(id) = model_id.as_deref() {
        let mm = app.state::<std::sync::Arc<crate::managers::model::ModelManager>>();
        let info = mm.get_model_info(id);
        crate::managers::transcription::translation::validate_translation_model(
            info.is_some(),
            info.as_ref().is_some_and(|m| {
                matches!(
                    m.engine_type,
                    crate::managers::model::EngineType::TranscribeCpp
                )
            }),
            info.as_ref().is_some_and(|m| m.supports_translation),
            true,
        )
        .map_err(|error| CommandError::new(CommandErrorCode::InvalidInput, error))?;
    }
    let mut settings = get_settings(&app);
    settings.translation_model_id = model_id;
    write_settings(&app, settings);
    Ok(())
}
