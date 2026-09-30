//! Repository for `transforms` (`data-model.md` §2): saved Command-Mode
//! prompts (P2 scope).

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension, Row};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq)]
pub struct Transform {
    /// uuid
    pub id: String,
    pub name: String,
    pub prompt: String,
    pub hotkey: Option<String>,
}

impl Transform {
    pub fn new(name: &str, prompt: &str) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name: name.to_string(),
            prompt: prompt.to_string(),
            hotkey: None,
        }
    }

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            name: row.get("name")?,
            prompt: row.get("prompt")?,
            hotkey: row.get("hotkey")?,
        })
    }
}

pub trait TransformRepository {
    fn create(&self, transform: &Transform) -> Result<()>;
    fn get(&self, id: &str) -> Result<Option<Transform>>;
    fn list(&self) -> Result<Vec<Transform>>;
    fn update(&self, transform: &Transform) -> Result<()>;
    fn delete(&self, id: &str) -> Result<()>;
}

pub struct SqliteTransformRepository<'a> {
    conn: &'a Connection,
}

impl<'a> SqliteTransformRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }
}

impl TransformRepository for SqliteTransformRepository<'_> {
    fn create(&self, transform: &Transform) -> Result<()> {
        self.conn.execute(
            "INSERT INTO transforms (id, name, prompt, hotkey) VALUES (?1,?2,?3,?4)",
            params![
                transform.id,
                transform.name,
                transform.prompt,
                transform.hotkey
            ],
        )?;
        Ok(())
    }

    fn get(&self, id: &str) -> Result<Option<Transform>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM transforms WHERE id = ?1")?;
        let transform = stmt
            .query_row(params![id], Transform::from_row)
            .optional()?;
        Ok(transform)
    }

    fn list(&self) -> Result<Vec<Transform>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM transforms ORDER BY name COLLATE NOCASE ASC")?;
        let rows = stmt.query_map([], Transform::from_row)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    fn update(&self, transform: &Transform) -> Result<()> {
        self.conn.execute(
            "UPDATE transforms SET name = ?1, prompt = ?2, hotkey = ?3 WHERE id = ?4",
            params![
                transform.name,
                transform.prompt,
                transform.hotkey,
                transform.id
            ],
        )?;
        Ok(())
    }

    fn delete(&self, id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM transforms WHERE id = ?1", params![id])?;
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
        let repo = SqliteTransformRepository::new(&conn);

        let mut t = Transform::new("Resumir", "Resuma o texto selecionado.");
        t.hotkey = Some("Ctrl+Alt+R".to_string());
        repo.create(&t).expect("create");

        let fetched = repo.get(&t.id).expect("get").expect("exists");
        assert_eq!(fetched, t);
        assert_eq!(repo.list().expect("list").len(), 1);

        let mut edited = fetched;
        edited.prompt = "Resuma em 3 bullets.".to_string();
        repo.update(&edited).expect("update");
        assert_eq!(
            repo.get(&t.id).expect("get").expect("exists").prompt,
            "Resuma em 3 bullets."
        );

        repo.delete(&t.id).expect("delete");
        assert!(repo.get(&t.id).expect("get").is_none());
    }
}
