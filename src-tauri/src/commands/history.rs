use crate::actions::process_transcription_output;
use crate::commands::{CommandError, CommandErrorCode, CommandResult};
use crate::managers::{
    history::{HistoryManager, PaginatedHistory},
    transcription::TranscriptionManager,
};
use std::sync::Arc;
use tauri::{AppHandle, State};

#[tauri::command]
#[specta::specta]
pub async fn get_history_entries(
    _app: AppHandle,
    history_manager: State<'_, Arc<HistoryManager>>,
    cursor: Option<i64>,
    limit: Option<usize>,
) -> CommandResult<PaginatedHistory> {
    history_manager
        .get_history_entries(cursor, limit)
        .await
        .map_err(|e| {
            CommandError::logged(
                CommandErrorCode::Internal,
                "Failed to load history entries",
                e,
            )
        })
}

#[tauri::command]
#[specta::specta]
pub async fn toggle_history_entry_saved(
    _app: AppHandle,
    history_manager: State<'_, Arc<HistoryManager>>,
    id: i64,
) -> CommandResult<()> {
    history_manager.toggle_saved_status(id).await.map_err(|e| {
        CommandError::logged(
            CommandErrorCode::Internal,
            "Failed to update history entry",
            e,
        )
    })
}

#[tauri::command]
#[specta::specta]
pub async fn get_audio_file_path(
    _app: AppHandle,
    history_manager: State<'_, Arc<HistoryManager>>,
    file_name: String,
) -> CommandResult<String> {
    let path = history_manager.get_audio_file_path(&file_name);
    path.to_str()
        .ok_or_else(|| CommandError::new(CommandErrorCode::InvalidInput, "Invalid audio file path"))
        .map(|s| s.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn delete_history_entry(
    _app: AppHandle,
    history_manager: State<'_, Arc<HistoryManager>>,
    id: i64,
) -> CommandResult<()> {
    history_manager.delete_entry(id).await.map_err(|e| {
        CommandError::logged(
            CommandErrorCode::Internal,
            "Failed to delete history entry",
            e,
        )
    })
}

#[tauri::command]
#[specta::specta]
pub async fn retry_history_entry_transcription(
    app: AppHandle,
    history_manager: State<'_, Arc<HistoryManager>>,
    transcription_manager: State<'_, Arc<TranscriptionManager>>,
    id: i64,
) -> CommandResult<()> {
    let entry = history_manager
        .get_entry_by_id(id)
        .await
        .map_err(|e| {
            CommandError::logged(
                CommandErrorCode::Internal,
                "Failed to load history entry",
                e,
            )
        })?
        .ok_or_else(|| CommandError::new(CommandErrorCode::NotFound, "History entry not found"))?;

    let audio_path = history_manager.get_audio_file_path(&entry.file_name);
    let samples = crate::audio_toolkit::read_wav_samples(&audio_path)
        .map_err(|e| CommandError::logged(CommandErrorCode::Internal, "Failed to load audio", e))?;

    if samples.is_empty() {
        return Err(CommandError::new(
            CommandErrorCode::InvalidInput,
            "Recording has no audio samples",
        ));
    }

    transcription_manager.initiate_model_load();

    let tm = Arc::clone(&transcription_manager);
    let transcription = tauri::async_runtime::spawn_blocking(move || tm.transcribe(samples))
        .await
        .map_err(|e| {
            CommandError::logged(CommandErrorCode::Internal, "Transcription task failed", e)
        })?
        .map_err(|e| CommandError::new(CommandErrorCode::Internal, e.to_string()))?;

    if transcription.is_empty() {
        return Err(CommandError::new(
            CommandErrorCode::InvalidInput,
            "Recording contains no speech",
        ));
    }

    let processed =
        process_transcription_output(&app, &transcription, entry.post_process_requested).await;
    history_manager
        .update_transcription(
            id,
            transcription,
            processed.post_processed_text,
            processed.post_process_prompt,
        )
        .map(|_| ())
        .map_err(|e| {
            CommandError::logged(
                CommandErrorCode::Internal,
                "Failed to update history entry",
                e,
            )
        })
}

#[tauri::command]
#[specta::specta]
pub async fn update_history_limit(
    app: AppHandle,
    history_manager: State<'_, Arc<HistoryManager>>,
    limit: usize,
) -> CommandResult<()> {
    let mut settings = crate::settings::get_settings(&app);
    settings.history_limit = limit;
    crate::settings::write_settings(&app, settings);

    history_manager.cleanup_old_entries().map_err(|e| {
        CommandError::logged(
            CommandErrorCode::Internal,
            "Failed to clean up history entries",
            e,
        )
    })?;

    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn update_recording_retention_period(
    app: AppHandle,
    history_manager: State<'_, Arc<HistoryManager>>,
    period: String,
) -> CommandResult<()> {
    use crate::settings::RecordingRetentionPeriod;

    let retention_period = match period.as_str() {
        "never" => RecordingRetentionPeriod::Never,
        "preserve_limit" => RecordingRetentionPeriod::PreserveLimit,
        "days3" => RecordingRetentionPeriod::Days3,
        "weeks2" => RecordingRetentionPeriod::Weeks2,
        "months3" => RecordingRetentionPeriod::Months3,
        _ => {
            return Err(CommandError::new(
                CommandErrorCode::InvalidInput,
                format!("Invalid retention period: {period}"),
            ))
        }
    };

    let mut settings = crate::settings::get_settings(&app);
    settings.recording_retention_period = retention_period;
    crate::settings::write_settings(&app, settings);

    history_manager.cleanup_old_entries().map_err(|e| {
        CommandError::logged(
            CommandErrorCode::Internal,
            "Failed to clean up history entries",
            e,
        )
    })?;

    Ok(())
}
