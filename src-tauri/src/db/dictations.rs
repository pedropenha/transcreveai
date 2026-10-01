//! Repository for the `dictations` table (`data-model.md` §2).
//!
//! This is the evolution of the inherited `transcription_history` table: the
//! migration preserves every legacy row and the repository is the single row
//! mapper used by `HistoryManager` and by future consumers (session queue,
//! history search).

use anyhow::{anyhow, Result};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension, Row};

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
    fn set_flagged(&self, id: i64, flagged: bool) -> Result<()>;
    fn delete(&self, id: i64) -> Result<()>;
    /// Newest-first listing; `cursor` is exclusive (`id < cursor`).
    /// `limit: None` means no LIMIT clause.
    fn list_desc(&self, cursor: Option<i64>, limit: Option<i64>) -> Result<Vec<Dictation>>;
    /// Latest entry regardless of content.
    fn latest(&self) -> Result<Option<Dictation>>;
    /// Latest entry with non-empty `raw_text`.
    fn latest_completed(&self) -> Result<Option<Dictation>>;
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
}
