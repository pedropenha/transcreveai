//! Repository for `dictionary_entries` (`data-model.md` §2): vocabulary hints
//! ('vocab') and ASR error corrections ('replacement').

use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension, Row};
use uuid::Uuid;

/// 'vocab' | 'replacement'
#[derive(Clone, Debug, PartialEq)]
pub struct DictionaryEntry {
    /// uuid
    pub id: String,
    /// Correct form, e.g. "Kubernetes", "Transcreve.ai".
    pub term: String,
    /// 'vocab' | 'replacement'
    pub kind: String,
    /// For 'replacement': what the ASR usually mishears (e.g. "cuber netes").
    pub match_text: Option<String>,
    pub case_sensitive: bool,
    /// 'manual' | 'auto'
    pub source: String,
    /// Unix epoch seconds.
    pub created_at: i64,
}

impl DictionaryEntry {
    /// `kind`: "vocab" or "replacement"; `match_text` only matters for
    /// replacements.
    pub fn new(term: &str, kind: &str, match_text: Option<&str>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            term: term.to_string(),
            kind: kind.to_string(),
            match_text: match_text.map(str::to_string),
            case_sensitive: false,
            source: "manual".to_string(),
            created_at: Utc::now().timestamp(),
        }
    }

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            term: row.get("term")?,
            kind: row.get("kind")?,
            match_text: row.get("match_text")?,
            case_sensitive: row.get("case_sensitive")?,
            source: row.get("source")?,
            created_at: row.get("created_at")?,
        })
    }
}

pub trait DictionaryRepository {
    fn create(&self, entry: &DictionaryEntry) -> Result<()>;
    fn get(&self, id: &str) -> Result<Option<DictionaryEntry>>;
    fn list(&self) -> Result<Vec<DictionaryEntry>>;
    fn list_by_kind(&self, kind: &str) -> Result<Vec<DictionaryEntry>>;
    fn update(&self, entry: &DictionaryEntry) -> Result<()>;
    fn delete(&self, id: &str) -> Result<()>;
}

pub struct SqliteDictionaryRepository<'a> {
    conn: &'a Connection,
}

impl<'a> SqliteDictionaryRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }
}

impl DictionaryRepository for SqliteDictionaryRepository<'_> {
    fn create(&self, entry: &DictionaryEntry) -> Result<()> {
        self.conn.execute(
            "INSERT INTO dictionary_entries (
                id, term, kind, match_text, case_sensitive, source, created_at
            ) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![
                entry.id,
                entry.term,
                entry.kind,
                entry.match_text,
                entry.case_sensitive,
                entry.source,
                entry.created_at,
            ],
        )?;
        Ok(())
    }

    fn get(&self, id: &str) -> Result<Option<DictionaryEntry>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM dictionary_entries WHERE id = ?1")?;
        let entry = stmt
            .query_row(params![id], DictionaryEntry::from_row)
            .optional()?;
        Ok(entry)
    }

    fn list(&self) -> Result<Vec<DictionaryEntry>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM dictionary_entries ORDER BY term COLLATE NOCASE ASC")?;
        let rows = stmt.query_map([], DictionaryEntry::from_row)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    fn list_by_kind(&self, kind: &str) -> Result<Vec<DictionaryEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT * FROM dictionary_entries WHERE kind = ?1
             ORDER BY term COLLATE NOCASE ASC",
        )?;
        let rows = stmt.query_map(params![kind], DictionaryEntry::from_row)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    fn update(&self, entry: &DictionaryEntry) -> Result<()> {
        self.conn.execute(
            "UPDATE dictionary_entries
             SET term = ?1, kind = ?2, match_text = ?3,
                 case_sensitive = ?4, source = ?5
             WHERE id = ?6",
            params![
                entry.term,
                entry.kind,
                entry.match_text,
                entry.case_sensitive,
                entry.source,
                entry.id,
            ],
        )?;
        Ok(())
    }

    fn delete(&self, id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM dictionary_entries WHERE id = ?1", params![id])?;
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
    fn crud_roundtrip() {
        let conn = setup();
        let repo = SqliteDictionaryRepository::new(&conn);

        let mut vocab = DictionaryEntry::new("Kubernetes", "vocab", None);
        vocab.source = "auto".to_string();
        let repl = DictionaryEntry::new("Transcreve.ai", "replacement", Some("transcreve ai"));

        repo.create(&vocab).expect("create vocab");
        repo.create(&repl).expect("create replacement");

        assert_eq!(repo.list().expect("list").len(), 2);
        assert_eq!(repo.list_by_kind("vocab").expect("by kind").len(), 1);

        let fetched = repo.get(&repl.id).expect("get").expect("exists");
        assert_eq!(fetched.match_text.as_deref(), Some("transcreve ai"));

        let mut updated = fetched.clone();
        updated.term = "Transcreve AI".to_string();
        updated.case_sensitive = true;
        repo.update(&updated).expect("update");
        let fetched = repo.get(&repl.id).expect("get").expect("exists");
        assert_eq!(fetched.term, "Transcreve AI");
        assert!(fetched.case_sensitive);

        repo.delete(&vocab.id).expect("delete");
        assert_eq!(repo.list().expect("list").len(), 1);
    }

    #[test]
    fn invalid_kind_is_rejected_by_check() {
        let conn = setup();
        let repo = SqliteDictionaryRepository::new(&conn);
        let e = DictionaryEntry::new("X", "bogus", None);
        assert!(repo.create(&e).is_err());
    }
}
