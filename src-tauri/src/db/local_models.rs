//! Repository for the `local_models` table (`data-model.md` §2): downloaded
//! on-device STT models (whisper/parakeet). `id` is the catalog id
//! (e.g. `whisper-large-v3-turbo-q5_0`), not a uuid.

use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension, Row};

/// 'downloading' | 'ready' | 'error'
#[derive(Clone, Debug, PartialEq)]
pub struct LocalModel {
    /// Catalog id.
    pub id: String,
    /// 'whisper' | 'parakeet' | …
    pub engine: String,
    pub file_path: String,
    pub size_bytes: i64,
    pub sha256: String,
    pub status: String,
    /// Unix epoch seconds; None while still downloading.
    pub downloaded_at: Option<i64>,
}

impl LocalModel {
    pub fn new(id: &str, engine: &str, file_path: &str, size_bytes: i64, sha256: &str) -> Self {
        Self {
            id: id.to_string(),
            engine: engine.to_string(),
            file_path: file_path.to_string(),
            size_bytes,
            sha256: sha256.to_string(),
            status: "downloading".to_string(),
            downloaded_at: None,
        }
    }

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            engine: row.get("engine")?,
            file_path: row.get("file_path")?,
            size_bytes: row.get("size_bytes")?,
            sha256: row.get("sha256")?,
            status: row.get("status")?,
            downloaded_at: row.get("downloaded_at")?,
        })
    }
}

pub trait LocalModelRepository {
    /// Insert or replace a catalog entry.
    fn save(&self, model: &LocalModel) -> Result<()>;
    fn get(&self, id: &str) -> Result<Option<LocalModel>>;
    fn list(&self) -> Result<Vec<LocalModel>>;
    /// 'ready' entries only.
    fn list_ready(&self) -> Result<Vec<LocalModel>>;
    /// Transition `status`; stamps `downloaded_at` when marking 'ready'.
    fn set_status(&self, id: &str, status: &str) -> Result<()>;
    fn delete(&self, id: &str) -> Result<()>;
}

pub struct SqliteLocalModelRepository<'a> {
    conn: &'a Connection,
}

impl<'a> SqliteLocalModelRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }
}

impl LocalModelRepository for SqliteLocalModelRepository<'_> {
    fn save(&self, model: &LocalModel) -> Result<()> {
        self.conn.execute(
            "INSERT INTO local_models (
                id, engine, file_path, size_bytes, sha256, status, downloaded_at
            ) VALUES (?1,?2,?3,?4,?5,?6,?7)
            ON CONFLICT(id) DO UPDATE SET
                engine = excluded.engine,
                file_path = excluded.file_path,
                size_bytes = excluded.size_bytes,
                sha256 = excluded.sha256,
                status = excluded.status,
                downloaded_at = excluded.downloaded_at",
            params![
                model.id,
                model.engine,
                model.file_path,
                model.size_bytes,
                model.sha256,
                model.status,
                model.downloaded_at,
            ],
        )?;
        Ok(())
    }

    fn get(&self, id: &str) -> Result<Option<LocalModel>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM local_models WHERE id = ?1")?;
        let model = stmt
            .query_row(params![id], LocalModel::from_row)
            .optional()?;
        Ok(model)
    }

    fn list(&self) -> Result<Vec<LocalModel>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM local_models ORDER BY id ASC")?;
        let rows = stmt.query_map([], LocalModel::from_row)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    fn list_ready(&self) -> Result<Vec<LocalModel>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM local_models WHERE status = 'ready' ORDER BY id ASC")?;
        let rows = stmt.query_map([], LocalModel::from_row)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    fn set_status(&self, id: &str, status: &str) -> Result<()> {
        let downloaded_at = if status == "ready" {
            Some(Utc::now().timestamp())
        } else {
            None
        };
        self.conn.execute(
            "UPDATE local_models SET status = ?1, downloaded_at = COALESCE(?2, downloaded_at)
             WHERE id = ?3",
            params![status, downloaded_at, id],
        )?;
        Ok(())
    }

    fn delete(&self, id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM local_models WHERE id = ?1", params![id])?;
        Ok(())
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

    #[test]
    fn save_get_lifecycle() {
        let conn = setup();
        let repo = SqliteLocalModelRepository::new(&conn);

        let m = LocalModel::new(
            "whisper-large-v3-turbo-q5_0",
            "whisper",
            "models/whisper/ggml.bin",
            1_500_000_000,
            "deadbeef",
        );
        repo.save(&m).expect("save");

        let fetched = repo.get(&m.id).expect("get").expect("exists");
        assert_eq!(fetched.status, "downloading");
        assert!(fetched.downloaded_at.is_none());
        assert!(repo.list_ready().expect("ready").is_empty());

        repo.set_status(&m.id, "ready").expect("ready");
        let fetched = repo.get(&m.id).expect("get").expect("exists");
        assert_eq!(fetched.status, "ready");
        assert!(fetched.downloaded_at.is_some());
        assert_eq!(repo.list_ready().expect("ready").len(), 1);

        repo.delete(&m.id).expect("delete");
        assert!(repo.get(&m.id).expect("get").is_none());
    }

    #[test]
    fn invalid_status_is_rejected_by_check() {
        let conn = setup();
        let repo = SqliteLocalModelRepository::new(&conn);
        let mut m = LocalModel::new("x", "whisper", "p", 1, "h");
        m.status = "bogus".to_string();
        assert!(repo.save(&m).is_err());
    }
}
