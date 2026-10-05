//! SQLite persistence layer for Transcreve.ai (`transcreve-ai.db`, WAL mode).
//!
//! The schema is defined in [`migrations`] and mirrors
//! `specs/architecture/data-model.md` §2. Data access follows the repository
//! pattern: each domain exposes a trait (`ProviderRepository`,
//! `DictationRepository`, …) implemented by a `Sqlite*Repository` that borrows
//! a [`Connection`]. Repositories take the connection as a parameter so callers
//! keep ownership of connection lifetime; tests exercise the same SQLite
//! implementation against `Connection::open_in_memory`.
//!
//! Only parameterized queries are allowed (ECC `rules/rust/security.md`).

pub mod app_profiles;
pub mod dictations;
pub mod dictionary;
pub mod local_models;
pub mod meeting_app_rules;
pub mod meeting_blocks;
pub mod meetings;
mod migrations;
pub mod notes;
pub mod providers;
pub mod snippets;
pub mod summary_templates;
pub mod transforms;

use anyhow::{anyhow, Result};
use log::{debug, info};
use rusqlite::Connection;
use rusqlite_migration::Migrations;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Database file prescribed by `specs/architecture/data-model.md` §1.
pub const DB_FILE_NAME: &str = "transcreve-ai.db";
/// Database file used by the inherited Handy schema; adopted on first run.
pub const LEGACY_DB_FILE_NAME: &str = "history.db";

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Resolve the database path inside `app_data_dir`, adopting the legacy
/// `history.db` file (and its WAL/SHM sidecars) when `transcreve-ai.db` does
/// not exist yet. Returns the path to `transcreve-ai.db` either way.
pub fn database_path(app_data_dir: &Path) -> Result<PathBuf> {
    let db_path = app_data_dir.join(DB_FILE_NAME);
    let legacy_path = app_data_dir.join(LEGACY_DB_FILE_NAME);

    if db_path.exists() || !legacy_path.exists() {
        return Ok(db_path);
    }

    fs::rename(&legacy_path, &db_path).map_err(|e| {
        anyhow!(
            "Failed to adopt legacy database {:?} as {:?}: {}",
            legacy_path,
            db_path,
            e
        )
    })?;

    // Move WAL/SHM/journal sidecars if they exist so no data is left behind.
    for suffix in ["-wal", "-shm", "-journal"] {
        let legacy_sidecar = app_data_dir.join(format!("{}{}", LEGACY_DB_FILE_NAME, suffix));
        let new_sidecar = app_data_dir.join(format!("{}{}", DB_FILE_NAME, suffix));
        if legacy_sidecar.exists() {
            fs::rename(&legacy_sidecar, &new_sidecar).map_err(|e| {
                anyhow!(
                    "Failed to adopt legacy database sidecar {:?}: {}",
                    legacy_sidecar,
                    e
                )
            })?;
        }
    }

    info!("Adopted legacy database {:?} as {:?}", legacy_path, db_path);
    Ok(db_path)
}

/// Open a database file with the pragmas the schema relies on
/// (WAL journal, foreign keys enforced, busy timeout).
pub fn open_connection(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    configure_connection(&conn)?;
    Ok(conn)
}

/// Apply the shared pragmas to an already-open connection (file or in-memory).
pub fn configure_connection(conn: &Connection) -> Result<()> {
    conn.busy_timeout(BUSY_TIMEOUT)?;
    let journal_mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
    debug!("SQLite journal_mode: {}", journal_mode);
    conn.pragma_update(None, "foreign_keys", "ON")?;
    Ok(())
}

/// Bring `conn` to the latest schema version.
///
/// First converts the tauri-plugin-sql `_sqlx_migrations` bookkeeping (if a
/// pre-fork database is present) into the `user_version` pragma that
/// `rusqlite_migration` uses, then applies every pending migration inside
/// transactions.
pub fn run_migrations(conn: &mut Connection) -> Result<()> {
    adopt_sqlx_migration_tracking(conn)?;

    let migrations = Migrations::new(migrations::MIGRATIONS.to_vec());
    migrations
        .validate()
        .map_err(|e| anyhow!("Invalid migrations: {}", e))?;

    let version_before: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    migrations.to_latest(conn)?;
    let version_after: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;

    if version_after > version_before {
        info!(
            "Database migrated from version {} to {}",
            version_before, version_after
        );
    } else {
        debug!("Database already at latest version {}", version_after);
    }

    Ok(())
}

/// Turn raw user input into a safe FTS5 `MATCH` expression (FR-009-25).
///
/// FTS5 parses its own query syntax — feeding the search box verbatim would
/// error on input like `foo "bar` or let the user inject operators. The
/// sanitizer keeps only alphanumeric runs (Unicode-aware, matching the
/// default `unicode61` tokenizer), quotes each run as a phrase and appends
/// `*` for prefix matching; terms are space-joined (implicit AND).
///
/// Returns `None` when nothing indexable remains — callers should then list
/// every row instead of running an empty `MATCH`.
pub(crate) fn fts_match_query(input: &str) -> Option<String> {
    let mut terms: Vec<String> = Vec::new();
    let mut current = String::new();
    for ch in input.chars() {
        if ch.is_alphanumeric() {
            current.push(ch);
        } else if !current.is_empty() {
            terms.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        terms.push(current);
    }
    if terms.is_empty() {
        return None;
    }
    Some(
        terms
            .iter()
            .map(|term| format!("\"{term}\"*"))
            .collect::<Vec<_>>()
            .join(" "),
    )
}

/// Migrate from tauri-plugin-sql's migration tracking to rusqlite_migration's.
/// tauri-plugin-sql used a `_sqlx_migrations` table, while rusqlite_migration
/// uses SQLite's `user_version` pragma. This sets `user_version` to the highest
/// successfully applied sqlx migration so those migrations don't re-run.
fn adopt_sqlx_migration_tracking(conn: &Connection) -> Result<()> {
    let has_sqlx_migrations: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='_sqlx_migrations'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(false);

    if !has_sqlx_migrations {
        return Ok(());
    }

    let current_version: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if current_version > 0 {
        // Already migrated to the rusqlite_migration system.
        return Ok(());
    }

    let old_version: i32 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations WHERE success = 1",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    if old_version > 0 {
        info!(
            "Migrating from tauri-plugin-sql (version {}) to rusqlite_migration",
            old_version
        );
        conn.pragma_update(None, "user_version", old_version)?;
        info!(
            "Migration tracking converted: user_version set to {}",
            old_version
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    fn migrated_conn() -> Connection {
        let mut conn = Connection::open_in_memory().expect("open in-memory db");
        configure_connection(&conn).expect("configure connection");
        run_migrations(&mut conn).expect("run migrations");
        conn
    }

    fn table_exists(conn: &Connection, name: &str) -> bool {
        conn.query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type IN ('table','view') AND name = ?1",
            params![name],
            |row| row.get(0),
        )
        .expect("check table existence")
    }

    #[test]
    fn migrations_on_empty_db_create_full_schema() {
        let conn = migrated_conn();

        for table in [
            "providers",
            "local_models",
            "dictations",
            "dictionary_entries",
            "snippets",
            "app_profiles",
            "transforms",
            "notes",
            "meetings",
            "meeting_segments",
            "meeting_blocks",
            "summary_templates",
            "meeting_app_rules",
            "dictations_fts",
            "notes_fts",
            "meeting_fts",
            "meetings_fts",
        ] {
            assert!(table_exists(&conn, table), "missing table {}", table);
        }

        // The legacy table must be gone after the rebuild.
        assert!(!table_exists(&conn, "transcription_history"));

        // All migrations applied: user_version equals the migration count.
        let version: i32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("read user_version");
        assert_eq!(version as usize, migrations::MIGRATIONS.len());
    }

    #[test]
    fn migrations_are_idempotent() {
        let mut conn = migrated_conn();
        run_migrations(&mut conn).expect("re-run migrations");

        let version: i32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("read user_version");
        assert_eq!(version as usize, migrations::MIGRATIONS.len());
    }

    /// Legacy schema at rusqlite_migration version 4 (the last Handy state).
    fn setup_legacy_v4_conn() -> Connection {
        let mut conn = Connection::open_in_memory().expect("open in-memory db");
        let legacy = Migrations::new(migrations::MIGRATIONS[..4].to_vec());
        legacy
            .to_latest(&mut conn)
            .expect("apply legacy migrations 1-4");
        conn
    }

    #[test]
    fn migration_from_legacy_v4_preserves_history() {
        let mut conn = setup_legacy_v4_conn();
        conn.execute(
            "INSERT INTO transcription_history (
                file_name, timestamp, saved, title, transcription_text,
                post_processed_text, post_process_prompt, post_process_requested
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                "rec-1.wav",
                100i64,
                true,
                "Recording 1",
                "ola mundo",
                Option::<String>::None,
                Option::<String>::None,
                false,
            ],
        )
        .expect("insert legacy row 1");
        conn.execute(
            "INSERT INTO transcription_history (
                file_name, timestamp, saved, title, transcription_text,
                post_processed_text, post_process_prompt, post_process_requested
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                "rec-2.wav",
                200i64,
                false,
                "Recording 2",
                "texto bruto",
                Some("texto final limpo"),
                Some("prompt usado"),
                true,
            ],
        )
        .expect("insert legacy row 2");

        run_migrations(&mut conn).expect("migrate to latest");

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM dictations", [], |row| row.get(0))
            .expect("count dictations");
        assert_eq!(count, 2);

        // Row without post-processing: final_text falls back to raw_text.
        let (raw, fin, audio, flagged, status): (String, String, Option<String>, bool, String) =
            conn.query_row(
                "SELECT raw_text, final_text, audio_path, flagged, status
                 FROM dictations WHERE id = 1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .expect("read dictation 1");
        assert_eq!(raw, "ola mundo");
        assert_eq!(fin, "ola mundo");
        assert_eq!(audio.as_deref(), Some("rec-1.wav"));
        assert!(flagged);
        assert_eq!(status, "inserted");

        // Row with post-processing keeps both texts and the prompt.
        let (raw, fin, instruction, pp): (String, String, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT raw_text, final_text, instruction, post_processed_text
                 FROM dictations WHERE id = 2",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("read dictation 2");
        assert_eq!(raw, "texto bruto");
        assert_eq!(fin, "texto final limpo");
        assert_eq!(instruction.as_deref(), Some("prompt usado"));
        assert_eq!(pp.as_deref(), Some("texto final limpo"));

        // autoincrement continues after the highest migrated id.
        let seq: i64 = conn
            .query_row(
                "SELECT seq FROM sqlite_sequence WHERE name = 'dictations'",
                [],
                |row| row.get(0),
            )
            .expect("read sqlite_sequence");
        assert!(seq >= 2);
    }

    #[test]
    fn migrated_rows_are_searchable_via_fts() {
        let mut conn = setup_legacy_v4_conn();
        conn.execute(
            "INSERT INTO transcription_history (
                file_name, timestamp, saved, title, transcription_text
            ) VALUES ('rec.wav', 100, 0, 'R', 'reuniao com kubernetes amanha')",
            [],
        )
        .expect("insert legacy row");

        run_migrations(&mut conn).expect("migrate to latest");

        let hits: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dictations_fts WHERE dictations_fts MATCH 'kubernetes'",
                [],
                |row| row.get(0),
            )
            .expect("fts query");
        assert_eq!(hits, 1);
    }

    #[test]
    fn fts_triggers_keep_dictations_index_in_sync() {
        let conn = migrated_conn();
        conn.execute(
            "INSERT INTO dictations (created_at, raw_text, final_text)
             VALUES (1, 'primeiro texto', 'primeiro texto')",
            [],
        )
        .expect("insert dictation");
        let hits: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dictations_fts WHERE dictations_fts MATCH 'primeiro'",
                [],
                |row| row.get(0),
            )
            .expect("fts after insert");
        assert_eq!(hits, 1);

        conn.execute(
            "UPDATE dictations SET raw_text = 'segundo texto', final_text = 'segundo texto'
             WHERE id = 1",
            [],
        )
        .expect("update dictation");
        let old_hits: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dictations_fts WHERE dictations_fts MATCH 'primeiro'",
                [],
                |row| row.get(0),
            )
            .expect("fts stale term");
        assert_eq!(old_hits, 0);
        let new_hits: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dictations_fts WHERE dictations_fts MATCH 'segundo'",
                [],
                |row| row.get(0),
            )
            .expect("fts updated term");
        assert_eq!(new_hits, 1);

        conn.execute("DELETE FROM dictations WHERE id = 1", [])
            .expect("delete dictation");
        let hits: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dictations_fts WHERE dictations_fts MATCH 'segundo'",
                [],
                |row| row.get(0),
            )
            .expect("fts after delete");
        assert_eq!(hits, 0);
    }

    #[test]
    fn fts_match_query_quotes_terms_and_prefixes() {
        // Each term becomes a quoted phrase with a prefix `*`; terms AND.
        assert_eq!(
            fts_match_query("reuniao kube").as_deref(),
            Some("\"reuniao\"* \"kube\"*")
        );
        // Unicode letters/digits are kept (unicode61 tokenizer).
        assert_eq!(
            fts_match_query("reunião 2fa").as_deref(),
            Some("\"reunião\"* \"2fa\"*")
        );
        // Punctuation, quotes and operator-ish words never reach the query —
        // the only unsafe characters are already stripped by the tokenizer.
        assert_eq!(
            fts_match_query("foo \"bar").as_deref(),
            Some("\"foo\"* \"bar\"*")
        );
        assert_eq!(
            fts_match_query("a-b (OR) NEAR").as_deref(),
            Some("\"a\"* \"b\"* \"OR\"* \"NEAR\"*")
        );
        // Nothing indexable left → None (caller lists everything).
        assert_eq!(fts_match_query(""), None);
        assert_eq!(fts_match_query("   \t\n"), None);
        assert_eq!(fts_match_query("\"-*()"), None);
    }

    #[test]
    fn fts_match_query_output_never_errors_against_real_table() {
        // Regression net for T-068: every sanitized query must parse as a
        // valid FTS5 MATCH expression, no matter how hostile the input was.
        let conn = migrated_conn();
        conn.execute(
            "INSERT INTO dictations (created_at, raw_text, final_text)
             VALUES (1, 'reuniao com kubernetes', 'reuniao com kubernetes')",
            [],
        )
        .expect("insert dictation");

        for raw in [
            "foo \"bar",
            "a AND OR NOT b",
            "NEAR(x y)",
            "reuniao:kube",
            "*%^&",
            "' single ' quotes \"",
        ] {
            let Some(query) = fts_match_query(raw) else {
                continue;
            };
            let result: rusqlite::Result<i64> = conn.query_row(
                "SELECT COUNT(*) FROM dictations_fts WHERE dictations_fts MATCH ?1",
                params![query],
                |row| row.get(0),
            );
            assert!(
                result.is_ok(),
                "sanitized query {query:?} (from {raw:?}) must parse"
            );
        }

        // And it actually matches (prefix semantics).
        let hits: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dictations_fts WHERE dictations_fts MATCH ?1",
                params![fts_match_query("reun kube").expect("query")],
                |row| row.get(0),
            )
            .expect("prefix match");
        assert_eq!(hits, 1);
    }

    /// Migration 14 rebuilds `dictations` to widen the `status` CHECK with
    /// `'routed'` (FR-012-12). Existing rows — ids included — must survive
    /// verbatim, and the FTS index + sync triggers must keep working.
    #[test]
    fn migration_14_preserves_rows_and_fts() {
        // Stop at version 13 (the last migration before the rebuild).
        let mut conn = Connection::open_in_memory().expect("open in-memory db");
        let up_to_13 = Migrations::new(migrations::MIGRATIONS[..13].to_vec());
        up_to_13
            .to_latest(&mut conn)
            .expect("apply migrations 1-13");
        conn.execute(
            "INSERT INTO dictations (created_at, raw_text, final_text)
             VALUES (100, 'texto preservado', 'texto preservado')",
            [],
        )
        .expect("insert dictation at v13");

        run_migrations(&mut conn).expect("migrate to latest");

        let (id, status): (i64, String) = conn
            .query_row(
                "SELECT id, status FROM dictations WHERE raw_text = 'texto preservado'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read migrated row");
        assert_eq!(id, 1);
        assert_eq!(status, "inserted");

        // The widened CHECK accepts 'routed'…
        conn.execute("UPDATE dictations SET status = 'routed' WHERE id = 1", [])
            .expect("routed status must be accepted after migration 14");

        // …the index is back…
        let index_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master
                 WHERE type = 'index' AND name = 'idx_dictations_created'",
                [],
                |row| row.get(0),
            )
            .expect("check index");
        assert!(index_exists);

        // …and the FTS shadow table + triggers still track the row.
        let hits: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dictations_fts WHERE dictations_fts MATCH 'preservado'",
                [],
                |row| row.get(0),
            )
            .expect("fts match after rebuild");
        assert_eq!(hits, 1);
        conn.execute(
            "UPDATE dictations SET raw_text = 'renomeado', final_text = 'renomeado'
             WHERE id = 1",
            [],
        )
        .expect("update after rebuild");
        let stale: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dictations_fts WHERE dictations_fts MATCH 'preservado'",
                [],
                |row| row.get(0),
            )
            .expect("fts stale check");
        assert_eq!(stale, 0);
    }

    /// Migration 15 adds the nullable `meetings.app_exe_path`. A meeting that
    /// already existed at v14 must survive intact (path NULL) and stay
    /// searchable; new rows can carry a path.
    #[test]
    fn migration_15_adds_nullable_app_exe_path_without_losing_meetings() {
        let mut conn = Connection::open_in_memory().expect("open in-memory db");
        configure_connection(&conn).expect("configure connection");
        Migrations::new(migrations::MIGRATIONS[..14].to_vec())
            .to_latest(&mut conn)
            .expect("apply migrations 1-14");
        conn.execute(
            "INSERT INTO meetings (id, title, app_exe, app_label, detection, status, started_at)
             VALUES ('m-old', 'Daily antiga', 'Zoom.exe', 'Zoom', 'auto_prompt', 'ready', 100)",
            [],
        )
        .expect("insert meeting at v14");

        run_migrations(&mut conn).expect("migrate to latest");

        let (title, exe, label, path): (String, String, String, Option<String>) = conn
            .query_row(
                "SELECT title, app_exe, app_label, app_exe_path FROM meetings WHERE id = 'm-old'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("read migrated meeting");
        assert_eq!(title, "Daily antiga");
        assert_eq!(exe, "Zoom.exe");
        assert_eq!(label, "Zoom");
        assert_eq!(path, None, "backfill-safe: old rows keep a NULL path");

        conn.execute(
            "UPDATE meetings SET app_exe_path = 'C:\\apps\\Zoom.exe' WHERE id = 'm-old'",
            [],
        )
        .expect("the column is writable");
        let hits: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM meetings_fts WHERE meetings_fts MATCH 'antiga'",
                [],
                |row| row.get(0),
            )
            .expect("fts still indexes the old meeting");
        assert_eq!(hits, 1);
    }

    /// Migration 16 adds the nullable `dictations.app_exe_path`. A dictation
    /// that already existed at v15 must survive intact (path NULL), its FTS
    /// index and created-at index must keep working, and new rows can carry
    /// the origin app.
    #[test]
    fn migration_16_adds_nullable_app_exe_path_without_losing_dictations() {
        let mut conn = Connection::open_in_memory().expect("open in-memory db");
        configure_connection(&conn).expect("configure connection");
        Migrations::new(migrations::MIGRATIONS[..15].to_vec())
            .to_latest(&mut conn)
            .expect("apply migrations 1-15");
        conn.execute(
            "INSERT INTO dictations (created_at, raw_text, final_text, status, app_exe, app_name)
             VALUES (100, 'texto antigo', 'texto antigo', 'inserted', 'Code.exe', 'Visual Studio Code')",
            [],
        )
        .expect("insert dictation at v15");

        run_migrations(&mut conn).expect("migrate to latest");

        let (text, exe, name, path): (String, String, String, Option<String>) = conn
            .query_row(
                "SELECT raw_text, app_exe, app_name, app_exe_path FROM dictations",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("read migrated dictation");
        assert_eq!(text, "texto antigo");
        assert_eq!(exe, "Code.exe");
        assert_eq!(name, "Visual Studio Code");
        assert_eq!(path, None, "old rows keep a NULL path");

        conn.execute(
            "INSERT INTO dictations (created_at, raw_text, final_text, app_exe_path)
             VALUES (200, 'texto novo', 'texto novo', 'C:/apps/Claude.exe')",
            [],
        )
        .expect("the column is writable and the FTS trigger still fires");
        for term in ["antigo", "novo"] {
            let hits: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM dictations_fts WHERE dictations_fts MATCH ?1",
                    [term],
                    |row| row.get(0),
                )
                .expect("fts query");
            assert_eq!(hits, 1, "{term}");
        }
        let plan: String = conn
            .query_row(
                "EXPLAIN QUERY PLAN SELECT id FROM dictations ORDER BY created_at DESC",
                [],
                |row| row.get(3),
            )
            .expect("query plan");
        assert!(plan.contains("idx_dictations_created"), "{plan}");
    }

    #[test]
    fn sqlx_tracking_is_adopted_without_rerunning_old_migrations() {
        let mut conn = Connection::open_in_memory().expect("open in-memory db");
        // Simulate a pre-fork DB: legacy schema created by sqlx migrations,
        // tracked in _sqlx_migrations instead of user_version.
        conn.execute_batch(
            "CREATE TABLE _sqlx_migrations (
                version INTEGER PRIMARY KEY,
                success BOOLEAN NOT NULL
            );
            INSERT INTO _sqlx_migrations (version, success) VALUES (1, 1), (2, 1), (3, 1), (4, 1);",
        )
        .expect("create sqlx tracking");
        // The legacy DDL exists already (applied by sqlx), but user_version is 0.
        conn.execute_batch(
            "CREATE TABLE transcription_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                file_name TEXT NOT NULL,
                timestamp INTEGER NOT NULL,
                saved BOOLEAN NOT NULL DEFAULT 0,
                title TEXT NOT NULL,
                transcription_text TEXT NOT NULL,
                post_processed_text TEXT,
                post_process_prompt TEXT,
                post_process_requested BOOLEAN NOT NULL DEFAULT 0
            );",
        )
        .expect("create legacy table");

        run_migrations(&mut conn).expect("run migrations");

        // Tracking adopted; only the new migrations ran on top of version 4.
        assert!(table_exists(&conn, "dictations"));
        assert!(!table_exists(&conn, "transcription_history"));
        let version: i32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("read user_version");
        assert_eq!(version as usize, migrations::MIGRATIONS.len());
    }

    #[test]
    fn database_path_adopts_legacy_file_and_sidecars() {
        let dir = tempfile::tempdir().expect("tempdir");
        let app_dir = dir.path();

        let legacy = app_dir.join(LEGACY_DB_FILE_NAME);
        fs::write(&legacy, b"sqlite-bytes").expect("write legacy db");
        fs::write(app_dir.join("history.db-wal"), b"wal").expect("write wal");
        fs::write(app_dir.join("history.db-shm"), b"shm").expect("write shm");

        let path = database_path(app_dir).expect("resolve database path");
        assert_eq!(path, app_dir.join(DB_FILE_NAME));
        assert!(path.exists());
        assert!(!legacy.exists());
        assert!(app_dir.join("transcreve-ai.db-wal").exists());
        assert!(app_dir.join("transcreve-ai.db-shm").exists());
        assert!(!app_dir.join("history.db-wal").exists());
    }

    #[test]
    fn database_path_prefers_existing_new_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let app_dir = dir.path();
        fs::write(app_dir.join(DB_FILE_NAME), b"new").expect("write new db");
        fs::write(app_dir.join(LEGACY_DB_FILE_NAME), b"old").expect("write legacy db");

        let path = database_path(app_dir).expect("resolve database path");
        assert_eq!(path, app_dir.join(DB_FILE_NAME));
        // The legacy file is left untouched rather than deleted.
        assert!(app_dir.join(LEGACY_DB_FILE_NAME).exists());
        assert_eq!(fs::read(&path).expect("read db"), b"new");
    }

    #[test]
    fn database_path_defaults_to_new_name_on_fresh_install() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = database_path(dir.path()).expect("resolve database path");
        assert_eq!(path, dir.path().join(DB_FILE_NAME));
    }
}
