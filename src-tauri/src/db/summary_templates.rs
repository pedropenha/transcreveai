//! Repository for `summary_templates` (`data-model.md` §2): prompts used to
//! summarize meetings.

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension, Row};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq)]
pub struct SummaryTemplate {
    /// uuid
    pub id: String,
    pub name: String,
    pub prompt: String,
    pub is_default: bool,
    /// Shipped with the app; not user-deletable.
    pub builtin: bool,
}

impl SummaryTemplate {
    pub fn new(name: &str, prompt: &str) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name: name.to_string(),
            prompt: prompt.to_string(),
            is_default: false,
            builtin: false,
        }
    }

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            name: row.get("name")?,
            prompt: row.get("prompt")?,
            is_default: row.get("is_default")?,
            builtin: row.get("builtin")?,
        })
    }
}

pub trait SummaryTemplateRepository {
    fn create(&self, template: &SummaryTemplate) -> Result<()>;
    fn get(&self, id: &str) -> Result<Option<SummaryTemplate>>;
    fn list(&self) -> Result<Vec<SummaryTemplate>>;
    /// Mark `id` as the default template, clearing every other one.
    fn set_default(&self, id: &str) -> Result<()>;
    fn delete(&self, id: &str) -> Result<()>;
}

pub struct SqliteSummaryTemplateRepository<'a> {
    conn: &'a Connection,
}

impl<'a> SqliteSummaryTemplateRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }
}

impl SummaryTemplateRepository for SqliteSummaryTemplateRepository<'_> {
    fn create(&self, template: &SummaryTemplate) -> Result<()> {
        self.conn.execute(
            "INSERT INTO summary_templates (id, name, prompt, is_default, builtin)
             VALUES (?1,?2,?3,?4,?5)",
            params![
                template.id,
                template.name,
                template.prompt,
                template.is_default,
                template.builtin
            ],
        )?;
        Ok(())
    }

    fn get(&self, id: &str) -> Result<Option<SummaryTemplate>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM summary_templates WHERE id = ?1")?;
        let template = stmt
            .query_row(params![id], SummaryTemplate::from_row)
            .optional()?;
        Ok(template)
    }

    fn list(&self) -> Result<Vec<SummaryTemplate>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM summary_templates ORDER BY name COLLATE NOCASE ASC")?;
        let rows = stmt.query_map([], SummaryTemplate::from_row)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    fn set_default(&self, id: &str) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("UPDATE summary_templates SET is_default = 0", [])?;
        tx.execute(
            "UPDATE summary_templates SET is_default = 1 WHERE id = ?1",
            params![id],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn delete(&self, id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM summary_templates WHERE id = ?1", params![id])?;
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
    fn crud_and_single_default() {
        let conn = setup();
        let repo = SqliteSummaryTemplateRepository::new(&conn);

        let a = SummaryTemplate::new("Padrão", "Resuma em tópicos.");
        let b = SummaryTemplate::new("Ata formal", "Escreva uma ata.");
        repo.create(&a).expect("create a");
        repo.create(&b).expect("create b");

        repo.set_default(&a.id).expect("default a");
        repo.set_default(&b.id).expect("default b");

        let all = repo.list().expect("list");
        let defaults: Vec<_> = all.iter().filter(|t| t.is_default).collect();
        assert_eq!(defaults.len(), 1);
        assert_eq!(defaults[0].id, b.id);

        repo.delete(&a.id).expect("delete");
        assert_eq!(repo.list().expect("list").len(), 1);
    }
}
