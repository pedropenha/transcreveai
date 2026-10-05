use crate::actions::process_transcription_output;
use crate::commands::{CommandError, CommandErrorCode, CommandResult};
use crate::managers::{
    history::{
        HistoryEntry, HistoryFilterOptions, HistoryManager, HistoryQuery, HistoryStatistics,
        PaginatedHistory,
    },
    transcription::TranscriptionManager,
};
use std::sync::Arc;
use tauri::{AppHandle, Manager, State};

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
pub async fn search_history_entries(
    history_manager: State<'_, Arc<HistoryManager>>,
    query: HistoryQuery,
) -> CommandResult<PaginatedHistory> {
    history_manager.search_history(query).await.map_err(|e| {
        CommandError::logged(
            CommandErrorCode::Internal,
            "Failed to search history entries",
            e,
        )
    })
}

#[tauri::command]
#[specta::specta]
pub async fn get_history_statistics(
    history_manager: State<'_, Arc<HistoryManager>>,
) -> CommandResult<HistoryStatistics> {
    history_manager.history_statistics().await.map_err(|e| {
        CommandError::logged(
            CommandErrorCode::Internal,
            "Failed to load history statistics",
            e,
        )
    })
}

#[tauri::command]
#[specta::specta]
pub async fn get_history_filter_options(
    history_manager: State<'_, Arc<HistoryManager>>,
) -> CommandResult<HistoryFilterOptions> {
    history_manager.history_filter_options().await.map_err(|e| {
        CommandError::logged(
            CommandErrorCode::Internal,
            "Failed to load history filters",
            e,
        )
    })
}

/// History row / detail: the origin app's icon as a `data:image/png` URI,
/// extracted from the executable captured at session start and cached per
/// exe path. `None` (never an error for a missing icon) when the entry
/// predates the stored path, the app has an embedded front-end logo, or the
/// extraction failed or exceeded its 2 s budget — the UI then shows a
/// monogram. The raw path never crosses IPC. Runs off the async workers: the
/// shell call can block.
#[tauri::command]
#[specta::specta]
pub async fn history_app_icon(
    history_manager: State<'_, Arc<HistoryManager>>,
    entry_id: i64,
) -> CommandResult<Option<String>> {
    let entry = history_manager
        .get_entry_by_id(entry_id)
        .await
        .map_err(|e| {
            CommandError::logged(
                CommandErrorCode::Internal,
                "Failed to load history entry",
                e,
            )
        })?
        .ok_or_else(|| CommandError::new(CommandErrorCode::NotFound, "History entry not found"))?;
    tokio::task::spawn_blocking(move || origin_icon(&entry, platform_icon))
        .await
        .map_err(|e| {
            CommandError::logged(CommandErrorCode::Internal, "Failed to load the app icon", e)
        })
}

/// Icon for the entry's origin app via `extract(label, path)`.
fn origin_icon(
    entry: &HistoryEntry,
    extract: impl Fn(&str, Option<&str>) -> Option<String>,
) -> Option<String> {
    let label = entry
        .app_name
        .as_deref()
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .or_else(|| {
            entry
                .app_exe
                .as_deref()
                .map(str::trim)
                .filter(|label| !label.is_empty())
        })?;
    extract(label, entry.app_exe_path.as_deref())
}

#[cfg(windows)]
fn platform_icon(label: &str, path: Option<&str>) -> Option<String> {
    crate::meeting::app_icon::dictation_icon(label, path)
}

/// Icon extraction is Windows-only for now (macOS lands with v1.0 packaging).
#[cfg(not(windows))]
fn platform_icon(_label: &str, _path: Option<&str>) -> Option<String> {
    None
}

#[cfg(test)]
mod origin_icon_tests {
    use super::*;
    use crate::db::dictations::Dictation;

    fn entry(app_name: Option<&str>, app_exe: Option<&str>, path: Option<&str>) -> HistoryEntry {
        HistoryEntry::from(Dictation {
            id: 1,
            created_at: 0,
            mode: "dictation".into(),
            duration_ms: 0,
            app_exe: app_exe.map(Into::into),
            app_name: app_name.map(Into::into),
            app_exe_path: path.map(Into::into),
            stt_provider_id: None,
            llm_provider_id: None,
            language: None,
            raw_text: "x".into(),
            final_text: "x".into(),
            instruction: None,
            status: "inserted".into(),
            error_code: None,
            latency_json: "{}".into(),
            audio_path: None,
            word_count: 1,
            flagged: false,
            post_processed_text: None,
            title: String::new(),
            post_process_requested: false,
        })
    }

    #[test]
    fn extracts_with_friendly_name_and_stored_path() {
        let e = entry(Some("Claude"), Some("Claude.exe"), Some(r"C:\a\Claude.exe"));
        let icon = origin_icon(&e, |label, path| {
            assert_eq!(label, "Claude");
            path.map(|p| format!("icon:{p}"))
        });
        assert_eq!(icon.as_deref(), Some(r"icon:C:\a\Claude.exe"));
    }

    #[test]
    fn falls_back_to_exe_name_as_label() {
        let e = entry(None, Some("tool.exe"), None);
        let icon = origin_icon(&e, |label, path| {
            assert_eq!(label, "tool.exe");
            assert_eq!(path, None);
            None
        });
        assert_eq!(icon, None);
    }

    #[test]
    fn blank_friendly_name_falls_back_to_executable() {
        let e = entry(Some("  "), Some("tool.exe"), Some(r"C:\a\tool.exe"));
        assert_eq!(
            origin_icon(&e, |label, _| Some(label.to_string())),
            Some("tool.exe".to_string())
        );
    }

    #[test]
    fn legacy_entries_without_app_never_extract() {
        let e = entry(None, None, None);
        assert_eq!(origin_icon(&e, |_, _| panic!("must not extract")), None);
        let blank = entry(Some("  "), None, None);
        assert_eq!(origin_icon(&blank, |_, _| panic!("must not extract")), None);
    }

    #[test]
    fn raw_path_is_never_serialized() {
        let e = entry(Some("Claude"), Some("Claude.exe"), Some(r"C:\a\Claude.exe"));
        let json = serde_json::to_value(&e).expect("serialize");
        assert!(json.get("app_exe_path").is_none());
        assert_eq!(json["app_name"], "Claude");
    }
}

#[tauri::command]
#[specta::specta]
pub async fn reinsert_history_entry(
    app: AppHandle,
    history_manager: State<'_, Arc<HistoryManager>>,
    id: i64,
) -> CommandResult<String> {
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
    if entry.final_text.trim().is_empty() {
        return Err(CommandError::new(
            CommandErrorCode::InvalidInput,
            "History entry has no text to insert",
        ));
    }
    let hub = app.get_webview_window(crate::window_labels::HUB);
    if let Some(window) = &hub {
        window.hide().map_err(|e| {
            CommandError::logged(CommandErrorCode::Internal, "Failed to release Hub focus", e)
        })?;
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }
    let report = crate::clipboard::paste(entry.final_text, app);
    if let Some(window) = hub {
        if let Err(error) = window.show() {
            log::warn!("Failed to restore Hub after reinsertion: {error}");
        }
    }
    if report.status == crate::insertion::InsertionStatus::Failed {
        return Err(CommandError::new(
            CommandErrorCode::Internal,
            report
                .error
                .unwrap_or_else(|| "Text insertion failed".to_string()),
        ));
    }
    Ok(report.status.as_str().to_string())
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
    retry_entry_transcription(&app, &history_manager, &transcription_manager, id).await
}

/// Re-transcribe the preserved recording of one history entry. Shared by the
/// history screen's retry button and the Flow Bar error state's
/// "Tentar novamente" (`flowbar_retry_last_failed`, AC-001-08).
pub(crate) async fn retry_entry_transcription(
    app: &AppHandle,
    history_manager: &HistoryManager,
    transcription_manager: &Arc<TranscriptionManager>,
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

    let tm = Arc::clone(transcription_manager);
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
        process_transcription_output(app, &transcription, entry.post_process_requested).await;
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
