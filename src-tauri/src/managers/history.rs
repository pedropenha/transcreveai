use anyhow::{anyhow, Result};
use chrono::{DateTime, Local, TimeZone, Utc};
use log::{debug, error, info};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use specta::Type;
use std::fs;
use std::path::PathBuf;
use tauri::AppHandle;
use tauri_specta::Event;

use crate::db::dictations::{
    Dictation, DictationProviderUsage, DictationQuery, DictationRepository, DictationStatistics,
    NewDictation, SqliteDictationRepository,
};
use crate::dictation_origin::DictationOrigin;

/// IPC shape of a dictation row, kept stable for the frontend.
/// Field names map onto the `dictations` table:
/// `file_name`→`audio_path`, `timestamp`→`created_at`, `saved`→`flagged`,
/// `transcription_text`→`raw_text`, `post_process_prompt`→`instruction`.
#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct PaginatedHistory {
    pub entries: Vec<HistoryEntry>,
    pub has_more: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type, tauri_specta::Event)]
#[serde(tag = "action")]
pub enum HistoryUpdatePayload {
    #[serde(rename = "added")]
    Added { entry: HistoryEntry },
    #[serde(rename = "updated")]
    Updated { entry: HistoryEntry },
    #[serde(rename = "deleted")]
    Deleted { id: i64 },
    #[serde(rename = "toggled")]
    Toggled { id: i64 },
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct HistoryEntry {
    pub id: i64,
    pub file_name: String,
    pub timestamp: i64,
    pub saved: bool,
    pub title: String,
    pub transcription_text: String,
    pub post_processed_text: Option<String>,
    pub post_process_prompt: Option<String>,
    pub post_process_requested: bool,
    pub mode: String,
    pub duration_ms: i64,
    pub app_exe: Option<String>,
    pub app_name: Option<String>,
    /// Full path of the origin app (migration 16) — only used to extract its
    /// icon; never sent over IPC (`serde(skip)`). Always stored sanitized.
    #[serde(skip)]
    pub app_exe_path: Option<String>,
    pub stt_provider_id: Option<String>,
    pub llm_provider_id: Option<String>,
    pub language: Option<String>,
    pub raw_text: String,
    pub final_text: String,
    pub status: String,
    pub error_code: Option<String>,
    pub latency_json: String,
    pub audio_available: bool,
    pub word_count: i64,
}

impl From<Dictation> for HistoryEntry {
    fn from(d: Dictation) -> Self {
        let audio_available = d.audio_path.is_some();
        Self {
            id: d.id,
            file_name: d.audio_path.unwrap_or_default(),
            timestamp: d.created_at,
            saved: d.flagged,
            title: d.title,
            transcription_text: d.raw_text.clone(),
            post_processed_text: d.post_processed_text,
            post_process_prompt: d.instruction,
            post_process_requested: d.post_process_requested,
            mode: d.mode,
            duration_ms: d.duration_ms,
            app_exe: d.app_exe,
            app_name: d.app_name,
            app_exe_path: d.app_exe_path,
            stt_provider_id: d.stt_provider_id,
            llm_provider_id: d.llm_provider_id,
            language: d.language,
            raw_text: d.raw_text,
            final_text: d.final_text,
            status: d.status,
            error_code: d.error_code,
            latency_json: d.latency_json,
            audio_available,
            word_count: d.word_count,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, Type)]
pub struct HistoryQuery {
    pub search: Option<String>,
    pub app: Option<String>,
    pub mode: Option<String>,
    pub status: Option<String>,
    pub from_timestamp: Option<i64>,
    pub to_timestamp: Option<i64>,
    pub cursor: Option<i64>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct HistoryProviderUsage {
    pub provider_id: String,
    pub duration_ms: i64,
    pub estimated_cost: f64,
}

impl From<DictationProviderUsage> for HistoryProviderUsage {
    fn from(usage: DictationProviderUsage) -> Self {
        Self {
            provider_id: usage.provider_id,
            duration_ms: usage.duration_ms,
            estimated_cost: usage.estimated_cost,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct HistoryStatistics {
    pub words_today: i64,
    pub words_week: i64,
    pub words_total: i64,
    pub duration_ms_total: i64,
    pub words_per_minute: f64,
    pub seconds_saved: f64,
    pub provider_usage: Vec<HistoryProviderUsage>,
}

impl From<DictationStatistics> for HistoryStatistics {
    fn from(stats: DictationStatistics) -> Self {
        Self {
            words_today: stats.words_today,
            words_week: stats.words_week,
            words_total: stats.words_total,
            duration_ms_total: stats.duration_ms_total,
            words_per_minute: stats.words_per_minute,
            seconds_saved: stats.seconds_saved,
            provider_usage: stats.provider_usage.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct HistoryFilterOptions {
    pub apps: Vec<String>,
}

/// Full dictation-session row (FR-002-18, AC-002-09): a `failed` session
/// keeps `status = "failed"`, the `error_code`, and the preserved `file_name`
/// so the history UI can offer a retry against the retained WAV.
#[derive(Clone, Debug, Default)]
pub struct SessionEntry {
    /// WAV file name inside the recordings directory (`None` when no audio
    /// was captured, e.g. a session aborted before recording started).
    pub file_name: Option<String>,
    pub raw_text: String,
    pub post_processed_text: Option<String>,
    pub post_process_prompt: Option<String>,
    pub post_process_requested: bool,
    /// `inserted` | `copied` | `failed` | `cancelled` | `saved_note`.
    pub status: String,
    pub error_code: Option<String>,
    /// Captured audio duration in milliseconds (0 when unknown).
    pub duration_ms: i64,
    pub language: Option<String>,
    pub stt_provider_id: Option<String>,
    /// Foreground app captured at session start (F010: "app de origem").
    pub origin: Option<DictationOrigin>,
}

pub struct HistoryManager {
    app_handle: AppHandle,
    recordings_dir: PathBuf,
    db_path: PathBuf,
}

impl HistoryManager {
    pub fn new(app_handle: &AppHandle) -> Result<Self> {
        // Create recordings directory in app data dir
        let app_data_dir = crate::portable::app_data_dir(app_handle)?;
        let recordings_dir = app_data_dir.join("recordings");
        // Adopts the legacy history.db file when transcreve-ai.db is absent.
        let db_path = crate::db::database_path(&app_data_dir)?;

        // Ensure recordings directory exists
        if !recordings_dir.exists() {
            fs::create_dir_all(&recordings_dir)?;
            debug!("Created recordings directory: {:?}", recordings_dir);
        }

        let manager = Self {
            app_handle: app_handle.clone(),
            recordings_dir,
            db_path,
        };

        // Initialize database and run migrations synchronously
        manager.init_database()?;

        Ok(manager)
    }

    fn init_database(&self) -> Result<()> {
        info!("Initializing database at {:?}", self.db_path);
        let mut conn = self.get_connection()?;
        crate::db::run_migrations(&mut conn)
    }

    fn get_connection(&self) -> Result<Connection> {
        crate::db::open_connection(&self.db_path)
    }

    pub fn recordings_dir(&self) -> &std::path::Path {
        &self.recordings_dir
    }

    /// Persist a full session row — including `failed` sessions whose WAV is
    /// kept for retry (AC-002-09). The WAV, when present, should already have
    /// been written to the recordings directory.
    pub fn save_session_entry(&self, session: SessionEntry) -> Result<HistoryEntry> {
        let timestamp = Utc::now().timestamp();
        let title = self.format_timestamp_title(timestamp);

        let conn = self.get_connection()?;
        let repo = SqliteDictationRepository::new(&conn);

        let mut new = NewDictation::new(session.raw_text, session.file_name);
        new.created_at = Some(timestamp);
        new.title = title;
        new.post_processed_text = session.post_processed_text;
        new.instruction = session.post_process_prompt;
        new.post_process_requested = session.post_process_requested;
        new.status = session.status;
        new.error_code = session.error_code;
        new.duration_ms = session.duration_ms;
        new.language = session.language;
        new.stt_provider_id = session.stt_provider_id;
        if let Some(origin) = session.origin {
            new.app_exe = Some(origin.exe_name);
            new.app_name = Some(origin.app_name);
            new.app_exe_path = origin.exe_path;
        }

        let entry = HistoryEntry::from(repo.insert(&new)?);

        debug!("Saved history entry with id {}", entry.id);

        self.cleanup_old_entries()?;

        // Emit typed event for real-time frontend updates
        if let Err(e) = (HistoryUpdatePayload::Added {
            entry: entry.clone(),
        })
        .emit(&self.app_handle)
        {
            error!("Failed to emit history-updated event: {}", e);
        }

        Ok(entry)
    }

    /// Update an existing history entry with new transcription results (used by retry).
    pub fn update_transcription(
        &self,
        id: i64,
        transcription_text: String,
        post_processed_text: Option<String>,
        post_process_prompt: Option<String>,
    ) -> Result<HistoryEntry> {
        let conn = self.get_connection()?;
        let repo = SqliteDictationRepository::new(&conn);

        repo.update_text(
            id,
            &transcription_text,
            post_processed_text.as_deref(),
            post_process_prompt.as_deref(),
            // A successful retry resolves a previous `failed` status
            // (AC-002-09) — the row rejoins the delivered history.
            "inserted",
        )
        .map_err(|e| anyhow!("Failed to update history entry {}: {}", id, e))?;

        let entry = HistoryEntry::from(
            repo.get(id)?
                .ok_or_else(|| anyhow!("History entry {} not found", id))?,
        );

        debug!("Updated transcription for history entry {}", id);

        if let Err(e) = (HistoryUpdatePayload::Updated {
            entry: entry.clone(),
        })
        .emit(&self.app_handle)
        {
            error!("Failed to emit history-updated event: {}", e);
        }

        Ok(entry)
    }

    /// Patch the provisional session row with the real insertion outcome
    /// (FR-005-11): `status` (`inserted`/`copied`/`failed`), the error when it
    /// failed, and `insert_ms`/`insert_method`/`insert_fallback` telemetry in
    /// `latency_json`.
    pub fn record_insertion_outcome(
        &self,
        id: i64,
        report: &crate::insertion::InsertionReport,
    ) -> Result<HistoryEntry> {
        self.patch_insertion_outcome(
            id,
            report.status.as_str(),
            report.error.as_deref(),
            Some(report.elapsed.as_millis() as i64),
            Some(report.plan.method_name()),
            report.clipboard_only_reason.map(|r| r.as_str()),
        )
    }

    /// Overwrite the provisional status of a session row whose pipeline ended
    /// without an insertion attempt (`cancelled`, or `failed` for an empty
    /// transcription).
    pub fn update_session_status(
        &self,
        id: i64,
        status: &str,
        error_code: Option<&str>,
    ) -> Result<HistoryEntry> {
        self.patch_insertion_outcome(id, status, error_code, None, None, None)
    }

    fn patch_insertion_outcome(
        &self,
        id: i64,
        status: &str,
        error_code: Option<&str>,
        insert_ms: Option<i64>,
        insert_method: Option<&str>,
        insert_fallback: Option<&str>,
    ) -> Result<HistoryEntry> {
        let conn = self.get_connection()?;
        let repo = SqliteDictationRepository::new(&conn);

        repo.update_insertion_outcome(
            id,
            status,
            error_code,
            insert_ms,
            insert_method,
            insert_fallback,
        )
        .map_err(|e| anyhow!("Failed to update insertion outcome of {}: {}", id, e))?;

        let entry = HistoryEntry::from(
            repo.get(id)?
                .ok_or_else(|| anyhow!("History entry {} not found", id))?,
        );

        if let Err(e) = (HistoryUpdatePayload::Updated {
            entry: entry.clone(),
        })
        .emit(&self.app_handle)
        {
            error!("Failed to emit history-updated event: {}", e);
        }

        Ok(entry)
    }

    pub fn cleanup_old_entries(&self) -> Result<()> {
        let retention_period = crate::settings::get_recording_retention_period(&self.app_handle);

        match retention_period {
            crate::settings::RecordingRetentionPeriod::Never => {
                // Don't delete anything
                Ok(())
            }
            crate::settings::RecordingRetentionPeriod::PreserveLimit => {
                // Use the old count-based logic with history_limit
                let limit = crate::settings::get_history_limit(&self.app_handle);
                self.cleanup_by_count(limit)
            }
            _ => {
                // Use time-based logic
                self.cleanup_by_time(retention_period)
            }
        }
    }

    fn delete_entries_and_files(&self, entries: &[Dictation]) -> Result<usize> {
        if entries.is_empty() {
            return Ok(0);
        }

        let conn = self.get_connection()?;
        let repo = SqliteDictationRepository::new(&conn);
        let mut deleted_count = 0;

        for entry in entries {
            // Delete database row
            repo.delete(entry.id)?;

            // Delete WAV file
            if let Some(file_name) = &entry.audio_path {
                let file_path = self.recordings_dir.join(file_name);
                if file_path.exists() {
                    if let Err(e) = fs::remove_file(&file_path) {
                        error!("Failed to delete WAV file {}: {}", file_name, e);
                    } else {
                        debug!("Deleted old WAV file: {}", file_name);
                        deleted_count += 1;
                    }
                }
            }
        }

        Ok(deleted_count)
    }

    fn cleanup_by_count(&self, limit: usize) -> Result<()> {
        let conn = self.get_connection()?;
        let repo = SqliteDictationRepository::new(&conn);

        // All unflagged entries, newest first
        let entries = repo.unflagged(None)?;

        if entries.len() > limit {
            let deleted_count = self.delete_entries_and_files(&entries[limit..])?;

            if deleted_count > 0 {
                debug!("Cleaned up {} old history entries by count", deleted_count);
            }
        }

        Ok(())
    }

    fn cleanup_by_time(
        &self,
        retention_period: crate::settings::RecordingRetentionPeriod,
    ) -> Result<()> {
        let conn = self.get_connection()?;
        let repo = SqliteDictationRepository::new(&conn);

        // Calculate cutoff timestamp (current time minus retention period)
        let now = Utc::now().timestamp();
        let cutoff_timestamp = match retention_period {
            crate::settings::RecordingRetentionPeriod::Days3 => now - (3 * 24 * 60 * 60), // 3 days in seconds
            crate::settings::RecordingRetentionPeriod::Weeks2 => now - (2 * 7 * 24 * 60 * 60), // 2 weeks in seconds
            crate::settings::RecordingRetentionPeriod::Months3 => now - (3 * 30 * 24 * 60 * 60), // 3 months in seconds (approximate)
            _ => unreachable!("Should not reach here"),
        };

        let entries = repo.unflagged(Some(cutoff_timestamp))?;
        let deleted_count = self.delete_entries_and_files(&entries)?;

        if deleted_count > 0 {
            debug!(
                "Cleaned up {} old history entries based on retention period",
                deleted_count
            );
        }

        Ok(())
    }

    pub async fn get_history_entries(
        &self,
        cursor: Option<i64>,
        limit: Option<usize>,
    ) -> Result<PaginatedHistory> {
        let conn = self.get_connection()?;
        let repo = SqliteDictationRepository::new(&conn);
        let limit = limit.map(|l| l.min(100));

        // Fetch one extra row to know whether a next page exists.
        let fetch_count = limit.map(|l| (l + 1) as i64);
        let mut entries: Vec<HistoryEntry> = repo
            .list_desc(cursor, fetch_count)?
            .into_iter()
            .map(HistoryEntry::from)
            .collect();

        let has_more = limit.is_some_and(|lim| entries.len() > lim);
        if has_more {
            entries.pop();
        }

        Ok(PaginatedHistory { entries, has_more })
    }

    pub async fn search_history(&self, query: HistoryQuery) -> Result<PaginatedHistory> {
        let conn = self.get_connection()?;
        let repo = SqliteDictationRepository::new(&conn);
        let limit = query.limit.unwrap_or(50).clamp(1, 100);
        let rows = repo.search(&DictationQuery {
            search: query.search,
            app: query.app,
            mode: query.mode,
            status: query.status,
            from_timestamp: query.from_timestamp,
            to_timestamp: query.to_timestamp,
            cursor: query.cursor,
            limit: limit + 1,
        })?;
        let has_more = rows.len() > limit;
        let entries = rows
            .into_iter()
            .take(limit)
            .map(HistoryEntry::from)
            .collect();
        Ok(PaginatedHistory { entries, has_more })
    }

    pub async fn history_statistics(&self) -> Result<HistoryStatistics> {
        let conn = self.get_connection()?;
        let repo = SqliteDictationRepository::new(&conn);
        let now = Local::now();
        let today_start = now
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .and_then(|midnight| Local.from_local_datetime(&midnight).earliest())
            .map(|midnight| midnight.timestamp())
            .unwrap_or_else(|| {
                let timestamp = Utc::now().timestamp();
                timestamp - timestamp.rem_euclid(86_400)
            });
        Ok(repo.statistics(today_start)?.into())
    }

    pub async fn history_filter_options(&self) -> Result<HistoryFilterOptions> {
        let conn = self.get_connection()?;
        let repo = SqliteDictationRepository::new(&conn);
        Ok(HistoryFilterOptions {
            apps: repo.distinct_apps()?,
        })
    }

    #[cfg(test)]
    fn get_latest_entry_with_conn(conn: &Connection) -> Result<Option<HistoryEntry>> {
        let repo = SqliteDictationRepository::new(conn);
        Ok(repo.latest()?.map(HistoryEntry::from))
    }

    /// Get the latest entry with non-empty transcription text — used by the
    /// "Colar última transcrição" shortcut (FR-002-19) to re-insert the most
    /// recent delivered `final_text`.
    pub fn get_latest_completed_entry(&self) -> Result<Option<HistoryEntry>> {
        let conn = self.get_connection()?;
        Self::get_latest_completed_entry_with_conn(&conn)
    }

    fn get_latest_completed_entry_with_conn(conn: &Connection) -> Result<Option<HistoryEntry>> {
        let repo = SqliteDictationRepository::new(conn);
        Ok(repo.latest_completed()?.map(HistoryEntry::from))
    }

    /// Get the newest `failed` dictation row — backs the Flow Bar error
    /// state's "Tentar novamente" button (AC-001-08), which re-transcribes the
    /// preserved recording through the same path as history's retry action.
    /// `None` when the last failure is too old to have kept its audio file is
    /// *not* filtered here — the caller checks `file_name` (still present as
    /// `Some` when the wav exists) before offering the retry.
    pub fn get_latest_failed_entry(&self) -> Result<Option<HistoryEntry>> {
        let conn = self.get_connection()?;
        let repo = SqliteDictationRepository::new(&conn);
        Ok(repo.latest_failed()?.map(HistoryEntry::from))
    }

    pub async fn toggle_saved_status(&self, id: i64) -> Result<()> {
        let conn = self.get_connection()?;
        let repo = SqliteDictationRepository::new(&conn);

        let entry = repo
            .get(id)?
            .ok_or_else(|| anyhow!("History entry {} not found", id))?;
        let new_saved = !entry.flagged;
        repo.set_flagged(id, new_saved)?;

        debug!("Toggled saved status for entry {}: {}", id, new_saved);

        // Emit history updated event
        if let Err(e) = (HistoryUpdatePayload::Toggled { id }).emit(&self.app_handle) {
            error!("Failed to emit history-updated event: {}", e);
        }

        Ok(())
    }

    pub fn get_audio_file_path(&self, file_name: &str) -> PathBuf {
        self.recordings_dir.join(file_name)
    }

    pub async fn get_entry_by_id(&self, id: i64) -> Result<Option<HistoryEntry>> {
        let conn = self.get_connection()?;
        let repo = SqliteDictationRepository::new(&conn);
        Ok(repo.get(id)?.map(HistoryEntry::from))
    }

    pub async fn delete_entry(&self, id: i64) -> Result<()> {
        let conn = self.get_connection()?;
        let repo = SqliteDictationRepository::new(&conn);

        // Get the entry to find the file name
        if let Some(entry) = repo.get(id)? {
            // Delete the audio file first
            if let Some(file_name) = &entry.audio_path {
                let file_path = self.get_audio_file_path(file_name);
                if file_path.exists() {
                    if let Err(e) = fs::remove_file(&file_path) {
                        error!("Failed to delete audio file {}: {}", file_name, e);
                        // Continue with database deletion even if file deletion fails
                    }
                }
            }
        }

        // Delete from database
        repo.delete(id)?;

        debug!("Deleted history entry with id: {}", id);

        // Emit history updated event
        if let Err(e) = (HistoryUpdatePayload::Deleted { id }).emit(&self.app_handle) {
            error!("Failed to emit history-updated event: {}", e);
        }

        Ok(())
    }

    fn format_timestamp_title(&self, timestamp: i64) -> String {
        if let Some(utc_datetime) = DateTime::from_timestamp(timestamp, 0) {
            // Convert UTC to local timezone
            let local_datetime = utc_datetime.with_timezone(&Local);
            local_datetime.format("%B %e, %Y - %l:%M%p").to_string()
        } else {
            format!("Recording {}", timestamp)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::run_migrations;

    fn setup_conn() -> Connection {
        let mut conn = Connection::open_in_memory().expect("open in-memory db");
        run_migrations(&mut conn).expect("run migrations");
        conn
    }

    fn insert_entry(conn: &Connection, timestamp: i64, text: &str, post_processed: Option<&str>) {
        let repo = SqliteDictationRepository::new(conn);
        let mut new = NewDictation::new(
            text.to_string(),
            Some(format!("transcreve-ai-{}.wav", timestamp)),
        );
        new.created_at = Some(timestamp);
        new.title = format!("Recording {}", timestamp);
        new.post_processed_text = post_processed.map(str::to_string);
        repo.insert(&new).expect("insert history entry");
    }

    #[test]
    fn get_latest_entry_returns_none_when_empty() {
        let conn = setup_conn();
        let entry = HistoryManager::get_latest_entry_with_conn(&conn).expect("fetch latest entry");
        assert!(entry.is_none());
    }

    #[test]
    fn get_latest_entry_returns_newest_entry() {
        let conn = setup_conn();
        insert_entry(&conn, 100, "first", None);
        insert_entry(&conn, 200, "second", Some("processed"));

        let entry = HistoryManager::get_latest_entry_with_conn(&conn)
            .expect("fetch latest entry")
            .expect("entry exists");

        assert_eq!(entry.timestamp, 200);
        assert_eq!(entry.transcription_text, "second");
        assert_eq!(entry.post_processed_text.as_deref(), Some("processed"));
    }

    #[test]
    fn get_latest_completed_entry_skips_empty_entries() {
        let conn = setup_conn();
        insert_entry(&conn, 100, "completed", None);
        insert_entry(&conn, 200, "", None);

        let entry = HistoryManager::get_latest_completed_entry_with_conn(&conn)
            .expect("fetch latest completed entry")
            .expect("completed entry exists");

        assert_eq!(entry.timestamp, 100);
        assert_eq!(entry.transcription_text, "completed");
    }
}
