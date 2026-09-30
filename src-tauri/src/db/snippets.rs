//! Repository for `snippets` (`data-model.md` §2): spoken triggers that expand
//! to text (`{date}`, `{time}`, `{clipboard}` placeholders supported).

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension, Row};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq)]
pub struct Snippet {
    /// uuid
    pub id: String,
    /// Spoken phrase (normalized at compare time); UNIQUE.
    pub trigger: String,
    pub expansion: String,
    /// 'whole' | 'inline'
    pub match_mode: String,
    pub enabled: bool,
    pub use_count: i64,
}

impl Snippet {
    pub fn new(trigger: &str, expansion: &str) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            trigger: trigger.to_string(),
            expansion: expansion.to_string(),
            match_mode: "inline".to_string(),
            enabled: true,
            use_count: 0,
        }
    }

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            trigger: row.get("trigger")?,
            expansion: row.get("expansion")?,
            match_mode: row.get("match_mode")?,
            enabled: row.get("enabled")?,
            use_count: row.get("use_count")?,
        })
    }
}

pub trait SnippetRepository {
    fn create(&self, snippet: &Snippet) -> Result<()>;
    fn get(&self, id: &str) -> Result<Option<Snippet>>;
    fn get_by_trigger(&self, trigger: &str) -> Result<Option<Snippet>>;
    fn list(&self) -> Result<Vec<Snippet>>;
    fn update(&self, snippet: &Snippet) -> Result<()>;
    /// Increment `use_count` (after an expansion fires).
    fn bump_use_count(&self, id: &str) -> Result<()>;
    fn delete(&self, id: &str) -> Result<()>;
}

pub struct SqliteSnippetRepository<'a> {
    conn: &'a Connection,
}

impl<'a> SqliteSnippetRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }
}

impl SnippetRepository for SqliteSnippetRepository<'_> {
    fn create(&self, snippet: &Snippet) -> Result<()> {
        self.conn.execute(
            "INSERT INTO snippets (id, trigger, expansion, match_mode, enabled, use_count)
             VALUES (?1,?2,?3,?4,?5,?6)",
            params![
                snippet.id,
                snippet.trigger,
                snippet.expansion,
                snippet.match_mode,
                snippet.enabled,
                snippet.use_count,
            ],
        )?;
        Ok(())
    }

    fn get(&self, id: &str) -> Result<Option<Snippet>> {
        let mut stmt = self.conn.prepare("SELECT * FROM snippets WHERE id = ?1")?;
        let snippet = stmt.query_row(params![id], Snippet::from_row).optional()?;
        Ok(snippet)
    }

    fn get_by_trigger(&self, trigger: &str) -> Result<Option<Snippet>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM snippets WHERE trigger = ?1")?;
        let snippet = stmt
            .query_row(params![trigger], Snippet::from_row)
            .optional()?;
        Ok(snippet)
    }

    fn list(&self) -> Result<Vec<Snippet>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM snippets ORDER BY trigger COLLATE NOCASE ASC")?;
        let rows = stmt.query_map([], Snippet::from_row)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    fn update(&self, snippet: &Snippet) -> Result<()> {
        self.conn.execute(
            "UPDATE snippets
             SET trigger = ?1, expansion = ?2, match_mode = ?3, enabled = ?4
             WHERE id = ?5",
            params![
                snippet.trigger,
                snippet.expansion,
                snippet.match_mode,
                snippet.enabled,
                snippet.id,
            ],
        )?;
        Ok(())
    }

    fn bump_use_count(&self, id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE snippets SET use_count = use_count + 1 WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    fn delete(&self, id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM snippets WHERE id = ?1", params![id])?;
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
    fn crud_and_use_count() {
        let conn = setup();
        let repo = SqliteSnippetRepository::new(&conn);

        let s = Snippet::new("meu email", "eu@exemplo.com");
        repo.create(&s).expect("create");

        let fetched = repo
            .get_by_trigger("meu email")
            .expect("get_by_trigger")
            .expect("exists");
        assert_eq!(fetched.expansion, "eu@exemplo.com");
        assert_eq!(fetched.match_mode, "inline");

        repo.bump_use_count(&s.id).expect("bump");
        repo.bump_use_count(&s.id).expect("bump");
        assert_eq!(repo.get(&s.id).expect("get").expect("exists").use_count, 2);

        let mut edited = fetched;
        edited.expansion = "novo@exemplo.com".to_string();
        edited.enabled = false;
        repo.update(&edited).expect("update");
        let fetched = repo.get(&s.id).expect("get").expect("exists");
        assert_eq!(fetched.expansion, "novo@exemplo.com");
        assert!(!fetched.enabled);

        repo.delete(&s.id).expect("delete");
        assert!(repo.list().expect("list").is_empty());
    }

    #[test]
    fn duplicate_trigger_is_rejected() {
        let conn = setup();
        let repo = SqliteSnippetRepository::new(&conn);
        repo.create(&Snippet::new("oi", "olá")).expect("create");
        assert!(repo.create(&Snippet::new("oi", "outro")).is_err());
    }
}
