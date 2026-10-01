//! Repository for the `dictations` table (`data-model.md` §2).
//!
//! This is the evolution of the inherited `transcription_history` table: the
//! migration preserves every legacy row and the repository is the single row
//! mapper used by `HistoryManager` and by future consumers (session queue,
//! history search).

use anyhow::{anyhow, Result};
use chrono::Utc;
use rusqlite::{params, params_from_iter, types::Value, Connection, OptionalExtension, Row};

/// Full column list in schema order — used by every SELECT so `from_row` is the
/// only place that knows the mapping.
const COLUMNS: &str = "id, created_at, mode, duration_ms, app_exe, app_name, \
     stt_provider_id, llm_provider_id, language, raw_text, final_text, \
     instruction, status, error_code, latency_json, audio_path, word_count, \
     flagged, post_processed_text, title, post_process_requested";

/// A row of `dictations`.
///
/// `status`, `mode` and the other TEXT-with-CHECK fields stay `String` in Rust
/// (the CHECK constraints enforce the domain); the spec lists the valid values.
#[derive(Clone, Debug, PartialEq)]
pub struct Dictation {
    pub id: i64,
    /// Unix epoch seconds.
    pub created_at: i64,
    /// 'dictation' | 'command' | 'note'
    pub mode: String,
    pub duration_ms: i64,
    pub app_exe: Option<String>,
    pub app_name: Option<String>,
    pub stt_provider_id: Option<String>,
    pub llm_provider_id: Option<String>,
    pub language: Option<String>,
    pub raw_text: String,
    /// Effective delivered text: post-processed when it ran, else `raw_text`.
    pub final_text: String,
    /// Spoken instruction (Command Mode) / post-process prompt (legacy rows).
    pub instruction: Option<String>,
    /// 'inserted' | 'copied' | 'failed' | 'cancelled' | 'saved_note'
    pub status: String,
    pub error_code: Option<String>,
    /// JSON object, e.g. {"stt_ms":…,"llm_ms":…,"insert_ms":…,"total_ms":…}
    pub latency_json: String,
    /// File name inside the recordings directory; NULL after retention expiry.
    pub audio_path: Option<String>,
    pub word_count: i64,
    /// User-pinned ("saved" in the legacy UI); excluded from retention cleanup.
    pub flagged: bool,
    // --- Fork-only columns kept for the inherited history UI ----------------
    /// Raw post-processor output when post-processing ran (distinct from
    /// `final_text`, which always holds the delivered text).
    pub post_processed_text: Option<String>,
    pub title: String,
    pub post_process_requested: bool,
}

/// Input for [`DictationRepository::insert`]. Fields beyond the legacy set are
/// pre-filled with the spec defaults by [`NewDictation::new`].
#[derive(Clone, Debug)]
pub struct NewDictation {
    pub raw_text: String,
    pub audio_path: Option<String>,
    pub title: String,
    pub post_processed_text: Option<String>,
    pub instruction: Option<String>,
    pub post_process_requested: bool,
    /// Defaults to `Utc::now()` when `None`.
    pub created_at: Option<i64>,
    pub mode: String,
    pub duration_ms: i64,
    pub app_exe: Option<String>,
    pub app_name: Option<String>,
    pub stt_provider_id: Option<String>,
    pub llm_provider_id: Option<String>,
    pub language: Option<String>,
    pub status: String,
    pub error_code: Option<String>,
    pub latency_json: String,
    pub flagged: bool,
}

impl NewDictation {
    /// A plain dictation entry — equivalent to what `HistoryManager::save_entry`
    /// stored under the legacy schema.
    pub fn new(raw_text: String, audio_path: Option<String>) -> Self {
        Self {
            raw_text,
            audio_path,
            title: String::new(),
            post_processed_text: None,
            instruction: None,
            post_process_requested: false,
            created_at: None,
            mode: "dictation".to_string(),
            duration_ms: 0,
            app_exe: None,
            app_name: None,
            stt_provider_id: None,
            llm_provider_id: None,
            language: None,
            status: "inserted".to_string(),
            error_code: None,
            latency_json: "{}".to_string(),
            flagged: false,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DictationQuery {
    pub search: Option<String>,
    pub app: Option<String>,
    pub mode: Option<String>,
    pub status: Option<String>,
    pub from_timestamp: Option<i64>,
    pub to_timestamp: Option<i64>,
    pub cursor: Option<i64>,
    pub limit: usize,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DictationProviderUsage {
    pub provider_id: String,
    pub duration_ms: i64,
    pub estimated_cost: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DictationStatistics {
    pub words_today: i64,
    pub words_week: i64,
    pub words_total: i64,
    pub duration_ms_total: i64,
    pub words_per_minute: f64,
    pub seconds_saved: f64,
    pub provider_usage: Vec<DictationProviderUsage>,
}

/// Persistence operations over `dictations`.
pub trait DictationRepository {
    /// Insert a new entry and return the stored row (id/created_at resolved).
    fn insert(&self, entry: &NewDictation) -> Result<Dictation>;
    fn get(&self, id: i64) -> Result<Option<Dictation>>;
    /// Replace the transcription results (retry path). `final_text` becomes
    /// `post_processed_text` when present, else `raw_text`. `status` is the
    /// resolved outcome (`inserted`/`copied`) and clears `error_code` — a
    /// retried `failed` row becomes a normal delivered entry (AC-002-09).
    fn update_text(
        &self,
        id: i64,
        raw_text: &str,
        post_processed_text: Option<&str>,
        instruction: Option<&str>,
        status: &str,
    ) -> Result<()>;
    /// Patch the insertion outcome of a provisional row (FR-005-11): the real
    /// `status` (`inserted`/`copied`/`failed`/`cancelled`), `error_code`, and
    /// insertion telemetry merged into `latency_json` (`insert_ms`,
    /// `insert_method`, `insert_fallback`). `None` telemetry fields leave the
    /// existing keys untouched.
    fn update_insertion_outcome(
        &self,
        id: i64,
        status: &str,
        error_code: Option<&str>,
        insert_ms: Option<i64>,
        insert_method: Option<&str>,
        insert_fallback: Option<&str>,
    ) -> Result<()>;
    fn set_flagged(&self, id: i64, flagged: bool) -> Result<()>;
    fn delete(&self, id: i64) -> Result<()>;
    /// Newest-first listing; `cursor` is exclusive (`id < cursor`).
    /// `limit: None` means no LIMIT clause.
    fn list_desc(&self, cursor: Option<i64>, limit: Option<i64>) -> Result<Vec<Dictation>>;
    fn search(&self, query: &DictationQuery) -> Result<Vec<Dictation>>;
    fn statistics(&self, today_start: i64) -> Result<DictationStatistics>;
    fn distinct_apps(&self) -> Result<Vec<String>>;
    /// Latest entry regardless of content.
    fn latest(&self) -> Result<Option<Dictation>>;
    /// Latest entry with non-empty `raw_text`.
    fn latest_completed(&self) -> Result<Option<Dictation>>;
    /// Latest `failed` row — backs the Flow Bar's "Tentar novamente"
    /// (AC-001-08). `audio_path` may be NULL once the recording retention
    /// cleanup ran; the caller must check before retrying.
    fn latest_failed(&self) -> Result<Option<Dictation>>;
    /// Unflagged entries (retention candidates), newest first.
    /// `older_than` restricts to `created_at < older_than`.
    fn unflagged(&self, older_than: Option<i64>) -> Result<Vec<Dictation>>;
}

pub struct SqliteDictationRepository<'a> {
    conn: &'a Connection,
}

impl<'a> SqliteDictationRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    fn map_row(row: &Row<'_>) -> rusqlite::Result<Dictation> {
        Ok(Dictation {
            id: row.get("id")?,
            created_at: row.get("created_at")?,
            mode: row.get("mode")?,
            duration_ms: row.get("duration_ms")?,
            app_exe: row.get("app_exe")?,
            app_name: row.get("app_name")?,
            stt_provider_id: row.get("stt_provider_id")?,
            llm_provider_id: row.get("llm_provider_id")?,
            language: row.get("language")?,
            raw_text: row.get("raw_text")?,
            final_text: row.get("final_text")?,
            instruction: row.get("instruction")?,
            status: row.get("status")?,
            error_code: row.get("error_code")?,
            latency_json: row.get("latency_json")?,
            audio_path: row.get("audio_path")?,
            word_count: row.get("word_count")?,
            flagged: row.get("flagged")?,
            post_processed_text: row.get("post_processed_text")?,
            title: row.get("title")?,
            post_process_requested: row.get("post_process_requested")?,
        })
    }

    fn word_count(text: &str) -> i64 {
        text.split_whitespace().count() as i64
    }
}

impl DictationRepository for SqliteDictationRepository<'_> {
    fn insert(&self, entry: &NewDictation) -> Result<Dictation> {
        let created_at = entry.created_at.unwrap_or_else(|| Utc::now().timestamp());
        let final_text = entry
            .post_processed_text
            .clone()
            .unwrap_or_else(|| entry.raw_text.clone());
        let word_count = Self::word_count(&final_text);

        self.conn.execute(
            "INSERT INTO dictations (
                created_at, mode, duration_ms, app_exe, app_name,
                stt_provider_id, llm_provider_id, language,
                raw_text, final_text, instruction, status, error_code,
                latency_json, audio_path, word_count, flagged,
                post_processed_text, title, post_process_requested
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20)",
            params![
                created_at,
                entry.mode,
                entry.duration_ms,
                entry.app_exe,
                entry.app_name,
                entry.stt_provider_id,
                entry.llm_provider_id,
                entry.language,
                entry.raw_text,
                final_text,
                entry.instruction,
                entry.status,
                entry.error_code,
                entry.latency_json,
                entry.audio_path,
                word_count,
                entry.flagged,
                entry.post_processed_text,
                entry.title,
                entry.post_process_requested,
            ],
        )?;

        let id = self.conn.last_insert_rowid();
        self.get(id)?
            .ok_or_else(|| anyhow!("Inserted dictation {} not found", id))
    }

    fn get(&self, id: i64) -> Result<Option<Dictation>> {
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT {} FROM dictations WHERE id = ?1", COLUMNS))?;
        let entry = stmt.query_row(params![id], Self::map_row).optional()?;
        Ok(entry)
    }

    fn update_text(
        &self,
        id: i64,
        raw_text: &str,
        post_processed_text: Option<&str>,
        instruction: Option<&str>,
        status: &str,
    ) -> Result<()> {
        let final_text = post_processed_text.unwrap_or(raw_text);
        let updated = self.conn.execute(
            "UPDATE dictations
             SET raw_text = ?1,
                 post_processed_text = ?2,
                 instruction = ?3,
                 final_text = ?4,
                 word_count = ?5,
                 status = ?6,
                 error_code = NULL
             WHERE id = ?7",
            params![
                raw_text,
                post_processed_text,
                instruction,
                final_text,
                Self::word_count(final_text),
                status,
                id
            ],
        )?;
        if updated == 0 {
            return Err(anyhow!("Dictation {} not found", id));
        }
        Ok(())
    }

    fn update_insertion_outcome(
        &self,
        id: i64,
        status: &str,
        error_code: Option<&str>,
        insert_ms: Option<i64>,
        insert_method: Option<&str>,
        insert_fallback: Option<&str>,
    ) -> Result<()> {
        let row = self
            .get(id)?
            .ok_or_else(|| anyhow!("Dictation {} not found", id))?;
        // Merge into the existing latency object instead of overwriting —
        // `stt_ms`/`llm_ms` may already be there (or the whole value may be
        // absent/legacy garbage, in which case we start from `{}`).
        let mut latency: serde_json::Value =
            serde_json::from_str::<serde_json::Value>(&row.latency_json)
                .ok()
                .filter(|v| v.is_object())
                .unwrap_or_else(|| serde_json::json!({}));
        if let Some(ms) = insert_ms {
            latency["insert_ms"] = ms.into();
        }
        if let Some(method) = insert_method {
            latency["insert_method"] = method.into();
        }
        if let Some(fallback) = insert_fallback {
            latency["insert_fallback"] = fallback.into();
        }
        let updated = self.conn.execute(
            "UPDATE dictations
             SET status = ?1, error_code = ?2, latency_json = ?3
             WHERE id = ?4",
            params![status, error_code, latency.to_string(), id],
        )?;
        if updated == 0 {
            return Err(anyhow!("Dictation {} not found", id));
        }
        Ok(())
    }

    fn set_flagged(&self, id: i64, flagged: bool) -> Result<()> {
        let updated = self.conn.execute(
            "UPDATE dictations SET flagged = ?1 WHERE id = ?2",
            params![flagged, id],
        )?;
        if updated == 0 {
            return Err(anyhow!("Dictation {} not found", id));
        }
        Ok(())
    }

    fn delete(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM dictations WHERE id = ?1", params![id])?;
        Ok(())
    }

    fn list_desc(&self, cursor: Option<i64>, limit: Option<i64>) -> Result<Vec<Dictation>> {
        let sql = format!(
            "SELECT {} FROM dictations
             WHERE (?1 IS NULL OR id < ?1)
             ORDER BY id DESC
             LIMIT ?2",
            COLUMNS
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![cursor, limit.unwrap_or(-1)], Self::map_row)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    fn search(&self, query: &DictationQuery) -> Result<Vec<Dictation>> {
        let mut clauses = Vec::new();
        let mut values = Vec::<Value>::new();
        let fts = query.search.as_deref().and_then(crate::db::fts_match_query);
        let join = if let Some(match_query) = fts {
            values.push(match_query.into());
            clauses.push(format!("dictations_fts MATCH ?{}", values.len()));
            " JOIN dictations_fts ON dictations_fts.rowid = d.rowid"
        } else {
            ""
        };

        if let Some(app) = query.app.as_deref().filter(|value| !value.is_empty()) {
            values.push(app.to_string().into());
            let index = values.len();
            clauses.push(format!("(d.app_exe = ?{index} OR d.app_name = ?{index})"));
        }
        for (column, value) in [
            ("mode", query.mode.as_deref()),
            ("status", query.status.as_deref()),
        ] {
            if let Some(value) = value.filter(|value| !value.is_empty()) {
                values.push(value.to_string().into());
                clauses.push(format!("d.{column} = ?{}", values.len()));
            }
        }
        for (operator, value) in [(">=", query.from_timestamp), ("<=", query.to_timestamp)] {
            if let Some(value) = value {
                values.push(value.into());
                clauses.push(format!("d.created_at {operator} ?{}", values.len()));
            }
        }
        if let Some(cursor) = query.cursor {
            let cursor_timestamp = self
                .conn
                .query_row(
                    "SELECT created_at FROM dictations WHERE id = ?1",
                    params![cursor],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?;
            if let Some(timestamp) = cursor_timestamp {
                values.push(timestamp.into());
                let timestamp_index = values.len();
                values.push(cursor.into());
                let id_index = values.len();
                clauses.push(format!(
                    "(d.created_at < ?{timestamp_index} OR (d.created_at = ?{timestamp_index} AND d.id < ?{id_index}))"
                ));
            }
        }

        let where_clause = if clauses.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", clauses.join(" AND "))
        };
        values.push((query.limit.clamp(1, 101) as i64).into());
        let qualified_columns = COLUMNS
            .split(',')
            .map(|column| format!("d.{}", column.trim()))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT {qualified_columns} FROM dictations d{join}{where_clause} \
             ORDER BY d.created_at DESC, d.id DESC LIMIT ?{}",
            values.len()
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(values), Self::map_row)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    fn statistics(&self, today_start: i64) -> Result<DictationStatistics> {
        let week = today_start - 6 * 86_400;
        let (words_today, words_week, words_total, duration_ms_total): (i64, i64, i64, i64) =
            self.conn.query_row(
                "SELECT
                    COALESCE(SUM(CASE WHEN created_at >= ?1 THEN word_count ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN created_at >= ?2 THEN word_count ELSE 0 END), 0),
                    COALESCE(SUM(word_count), 0),
                    COALESCE(SUM(duration_ms), 0)
                 FROM dictations",
                params![today_start, week],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?;
        let words_per_minute = if duration_ms_total > 0 {
            words_total as f64 * 60_000.0 / duration_ms_total as f64
        } else {
            0.0
        };
        let seconds_saved =
            (words_total as f64 * 60.0 / 40.0 - duration_ms_total as f64 / 1_000.0).max(0.0);
        let mut stmt = self.conn.prepare(
            "SELECT COALESCE(stt_provider_id, 'local'), COALESCE(SUM(duration_ms), 0)
             FROM dictations GROUP BY COALESCE(stt_provider_id, 'local') ORDER BY 1",
        )?;
        let provider_usage = stmt
            .query_map([], |row| {
                Ok(DictationProviderUsage {
                    provider_id: row.get(0)?,
                    duration_ms: row.get(1)?,
                    estimated_cost: 0.0,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(DictationStatistics {
            words_today,
            words_week,
            words_total,
            duration_ms_total,
            words_per_minute,
            seconds_saved,
            provider_usage,
        })
    }

    fn distinct_apps(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT app FROM (
                SELECT app_name AS app FROM dictations WHERE app_name IS NOT NULL AND app_name != ''
                UNION
                SELECT app_exe AS app FROM dictations WHERE app_exe IS NOT NULL AND app_exe != ''
             ) ORDER BY app COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    fn latest(&self) -> Result<Option<Dictation>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {} FROM dictations ORDER BY created_at DESC, id DESC LIMIT 1",
            COLUMNS
        ))?;
        let entry = stmt.query_row([], Self::map_row).optional()?;
        Ok(entry)
    }

    fn latest_completed(&self) -> Result<Option<Dictation>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {} FROM dictations WHERE raw_text != ''
             ORDER BY created_at DESC, id DESC LIMIT 1",
            COLUMNS
        ))?;
        let entry = stmt.query_row([], Self::map_row).optional()?;
        Ok(entry)
    }

    fn latest_failed(&self) -> Result<Option<Dictation>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {} FROM dictations WHERE status = 'failed'
             ORDER BY created_at DESC, id DESC LIMIT 1",
            COLUMNS
        ))?;
        let entry = stmt.query_row([], Self::map_row).optional()?;
        Ok(entry)
    }

    fn unflagged(&self, older_than: Option<i64>) -> Result<Vec<Dictation>> {
        let sql = format!(
            "SELECT {} FROM dictations
             WHERE flagged = 0 AND (?1 IS NULL OR created_at < ?1)
             ORDER BY created_at DESC",
            COLUMNS
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![older_than], Self::map_row)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::run_migrations;

    fn setup() -> Connection {
        let mut conn = Connection::open_in_memory().expect("open in-memory db");
        run_migrations(&mut conn).expect("run migrations");
        conn
    }

    fn repo(conn: &Connection) -> SqliteDictationRepository<'_> {
        SqliteDictationRepository::new(conn)
    }

    #[test]
    fn insert_and_get_roundtrip() {
        let conn = setup();
        let repo = repo(&conn);

        let mut new = NewDictation::new("ola mundo".to_string(), Some("a.wav".to_string()));
        new.created_at = Some(123);
        new.title = "Title".to_string();
        new.post_processed_text = Some("Olá, mundo.".to_string());
        new.instruction = Some("fix".to_string());
        new.post_process_requested = true;

        let d = repo.insert(&new).expect("insert");
        assert!(d.id > 0);
        assert_eq!(d.created_at, 123);
        assert_eq!(d.final_text, "Olá, mundo.");
        assert_eq!(d.word_count, 2);
        assert_eq!(d.status, "inserted");
        assert_eq!(d.mode, "dictation");

        let fetched = repo.get(d.id).expect("get").expect("exists");
        assert_eq!(fetched, d);
    }

    #[test]
    fn update_text_swaps_final_text_and_word_count() {
        let conn = setup();
        let repo = repo(&conn);
        let d = repo
            .insert(&NewDictation::new("um dois".to_string(), None))
            .expect("insert");

        repo.update_text(
            d.id,
            "um dois tres",
            Some("Um, dois, três."),
            None,
            "inserted",
        )
        .expect("update_text");
        let d = repo.get(d.id).expect("get").expect("exists");
        assert_eq!(d.raw_text, "um dois tres");
        assert_eq!(d.final_text, "Um, dois, três.");
        assert_eq!(d.word_count, 3);

        // Without post-processing, final_text falls back to raw.
        repo.update_text(d.id, "novo", None, None, "inserted")
            .expect("update_text");
        let d = repo.get(d.id).expect("get").expect("exists");
        assert_eq!(d.final_text, "novo");
        assert!(d.post_processed_text.is_none());

        assert!(repo.update_text(999, "x", None, None, "inserted").is_err());
    }

    #[test]
    fn update_text_recovers_a_failed_row() {
        let conn = setup();
        let repo = repo(&conn);

        let mut n = NewDictation::new(String::new(), Some("lost.wav".to_string()));
        n.status = "failed".to_string();
        n.error_code = Some("stt unavailable".to_string());
        let d = repo.insert(&n).expect("insert");
        assert_eq!(d.status, "failed");
        assert_eq!(d.error_code.as_deref(), Some("stt unavailable"));

        repo.update_text(d.id, "cheguei", None, None, "inserted")
            .expect("update_text");
        let d = repo.get(d.id).expect("get").expect("exists");
        assert_eq!(d.status, "inserted");
        assert_eq!(d.error_code, None);
        assert_eq!(d.final_text, "cheguei");
    }

    /// FR-005-11: the provisional `inserted` row is patched with the real
    /// insertion outcome, and telemetry lands in `latency_json` without
    /// clobbering pre-existing keys.
    #[test]
    fn update_insertion_outcome_patches_status_and_latency() {
        let conn = setup();
        let repo = repo(&conn);
        let mut n = NewDictation::new("ola".to_string(), None);
        n.latency_json = r#"{"stt_ms":123}"#.to_string();
        let d = repo.insert(&n).expect("insert");
        assert_eq!(d.status, "inserted");

        repo.update_insertion_outcome(
            d.id,
            "copied",
            None,
            Some(7),
            Some("clipboard_only"),
            Some("elevated_target"),
        )
        .expect("update outcome");

        let d = repo.get(d.id).expect("get").expect("exists");
        assert_eq!(d.status, "copied");
        assert_eq!(d.error_code, None);
        let latency: serde_json::Value =
            serde_json::from_str(&d.latency_json).expect("valid latency_json");
        assert_eq!(latency["stt_ms"], 123); // pre-existing key survives
        assert_eq!(latency["insert_ms"], 7);
        assert_eq!(latency["insert_method"], "clipboard_only");
        assert_eq!(latency["insert_fallback"], "elevated_target");
    }

    #[test]
    fn update_insertion_outcome_records_failure() {
        let conn = setup();
        let repo = repo(&conn);
        let d = repo
            .insert(&NewDictation::new("ola".to_string(), None))
            .expect("insert");

        repo.update_insertion_outcome(
            d.id,
            "failed",
            Some("clipboard locked"),
            Some(42),
            Some("paste"),
            None,
        )
        .expect("update outcome");

        let d = repo.get(d.id).expect("get").expect("exists");
        assert_eq!(d.status, "failed");
        assert_eq!(d.error_code.as_deref(), Some("clipboard locked"));
        let latency: serde_json::Value =
            serde_json::from_str(&d.latency_json).expect("valid latency_json");
        assert_eq!(latency["insert_ms"], 42);
        assert_eq!(latency["insert_method"], "paste");
        assert!(latency.get("insert_fallback").is_none());

        assert!(repo
            .update_insertion_outcome(999, "failed", None, None, None, None)
            .is_err());
    }

    /// A row whose `latency_json` is not a JSON object (legacy garbage)
    /// still gets patched — telemetry starts from `{}`.
    #[test]
    fn update_insertion_outcome_tolerates_non_object_latency() {
        let conn = setup();
        let repo = repo(&conn);
        let mut n = NewDictation::new("ola".to_string(), None);
        n.latency_json = "not json".to_string();
        let d = repo.insert(&n).expect("insert");

        repo.update_insertion_outcome(d.id, "inserted", None, Some(3), Some("type"), None)
            .expect("update outcome");
        let d = repo.get(d.id).expect("get").expect("exists");
        let latency: serde_json::Value =
            serde_json::from_str(&d.latency_json).expect("valid latency_json");
        assert_eq!(latency["insert_method"], "type");
    }

    #[test]
    fn latest_failed_returns_newest_failed_row() {
        let conn = setup();
        let repo = repo(&conn);

        // Empty table → nothing to retry (AC-001-08 hides the affordance).
        assert_eq!(repo.latest_failed().expect("latest_failed"), None);

        repo.insert(&NewDictation::new("ok".to_string(), None))
            .expect("insert ok");
        let mut n = NewDictation::new(String::new(), Some("lost.wav".to_string()));
        n.status = "failed".to_string();
        n.error_code = Some("stt unavailable".to_string());
        let failed = repo.insert(&n).expect("insert failed");
        // A newer non-failed row must not win over the failed one.
        repo.insert(&NewDictation::new("ok2".to_string(), None))
            .expect("insert ok2");

        let found = repo.latest_failed().expect("latest_failed").expect("some");
        assert_eq!(found.id, failed.id);
        assert_eq!(found.error_code.as_deref(), Some("stt unavailable"));
    }

    #[test]
    fn list_desc_paginates_with_cursor() {
        let conn = setup();
        let repo = repo(&conn);
        for i in 0..5 {
            let mut n = NewDictation::new(format!("t{}", i), None);
            n.created_at = Some(i);
            repo.insert(&n).expect("insert");
        }

        let page1 = repo.list_desc(None, Some(2)).expect("page1");
        assert_eq!(page1.len(), 2);
        assert_eq!(page1[0].id, 5);

        let page2 = repo.list_desc(Some(page1[1].id), Some(2)).expect("page2");
        assert_eq!(page2.len(), 2);
        assert_eq!(page2[0].id, 3);

        let rest = repo.list_desc(Some(page2[1].id), None).expect("rest");
        assert_eq!(rest.len(), 1);
    }

    #[test]
    fn flagged_and_unflagged_filtering() {
        let conn = setup();
        let repo = repo(&conn);
        let a = repo
            .insert(&NewDictation::new("a".to_string(), None))
            .expect("insert");
        repo.set_flagged(a.id, true).expect("flag");

        let unflagged = repo.unflagged(None).expect("unflagged");
        assert!(unflagged.iter().all(|d| !d.flagged));
        assert!(unflagged.iter().all(|d| d.id != a.id));
    }

    #[test]
    fn delete_removes_row() {
        let conn = setup();
        let repo = repo(&conn);
        let d = repo
            .insert(&NewDictation::new("x".to_string(), None))
            .expect("insert");
        repo.delete(d.id).expect("delete");
        assert!(repo.get(d.id).expect("get").is_none());
    }

    #[test]
    fn search_uses_fts_and_combines_filters() {
        let conn = setup();
        let repo = repo(&conn);

        let mut matching = NewDictation::new("plano bruto".to_string(), None);
        matching.created_at = Some(200);
        matching.mode = "command".to_string();
        matching.app_name = Some("Visual Studio Code".to_string());
        matching.app_exe = Some("Code.exe".to_string());
        matching.status = "copied".to_string();
        matching.post_processed_text = Some("Plano da entrega revisado".to_string());
        repo.insert(&matching).expect("insert matching");

        let mut wrong_status = NewDictation::new("Plano da entrega antigo".to_string(), None);
        wrong_status.created_at = Some(150);
        wrong_status.mode = "command".to_string();
        wrong_status.app_name = Some("Visual Studio Code".to_string());
        wrong_status.status = "failed".to_string();
        repo.insert(&wrong_status).expect("insert wrong status");

        let rows = repo
            .search(&DictationQuery {
                search: Some("entrega revis".to_string()),
                app: Some("Code.exe".to_string()),
                mode: Some("command".to_string()),
                status: Some("copied".to_string()),
                from_timestamp: Some(180),
                to_timestamp: Some(220),
                cursor: None,
                limit: 20,
            })
            .expect("search");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].final_text, "Plano da entrega revisado");
    }

    #[test]
    fn search_cursor_follows_created_at_then_id_order() {
        let conn = setup();
        let repo = repo(&conn);
        for (created_at, text) in [(300, "newest"), (100, "oldest"), (200, "middle")] {
            let mut entry = NewDictation::new(text.to_string(), None);
            entry.created_at = Some(created_at);
            repo.insert(&entry).expect("insert cursor row");
        }

        let first = repo
            .search(&DictationQuery {
                limit: 2,
                ..DictationQuery::default()
            })
            .expect("first page");
        assert_eq!(
            first
                .iter()
                .map(|row| row.raw_text.as_str())
                .collect::<Vec<_>>(),
            ["newest", "middle"]
        );
        let second = repo
            .search(&DictationQuery {
                cursor: Some(first[1].id),
                limit: 2,
                ..DictationQuery::default()
            })
            .expect("second page");
        assert_eq!(
            second
                .iter()
                .map(|row| row.raw_text.as_str())
                .collect::<Vec<_>>(),
            ["oldest"]
        );
    }

    #[test]
    fn statistics_cover_today_week_total_and_speaking_speed() {
        let conn = setup();
        let repo = repo(&conn);
        let now = 1_700_000_000;

        for (created_at, words, duration_ms) in [
            (now - 60, "um dois tres quatro", 2_000),
            (now - 3 * 86_400, "cinco seis", 3_000),
            (now - 20 * 86_400, "sete oito nove", 4_000),
        ] {
            let mut entry = NewDictation::new(words.to_string(), None);
            entry.created_at = Some(created_at);
            entry.duration_ms = duration_ms;
            repo.insert(&entry).expect("insert stats row");
        }

        let today_start = now - now.rem_euclid(86_400);
        let stats = repo.statistics(today_start).expect("statistics");
        assert_eq!(stats.words_today, 4);
        assert_eq!(stats.words_week, 6);
        assert_eq!(stats.words_total, 9);
        assert_eq!(stats.duration_ms_total, 9_000);
        assert!((stats.words_per_minute - 60.0).abs() < f64::EPSILON);
        assert!(stats.seconds_saved > 4.0);
        assert_eq!(stats.provider_usage.len(), 1);
        assert_eq!(stats.provider_usage[0].provider_id, "local");
        assert_eq!(stats.provider_usage[0].duration_ms, 9_000);
        assert_eq!(stats.provider_usage[0].estimated_cost, 0.0);
    }
}
