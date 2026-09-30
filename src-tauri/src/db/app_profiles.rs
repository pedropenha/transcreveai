//! Repository for `app_profiles` (`data-model.md` §2): per-application style
//! profiles matched by exe name / window-title regex.

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension, Row};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq)]
pub struct AppProfile {
    /// uuid
    pub id: String,
    pub name: String,
    /// e.g. "slack.exe" (case-insensitive match is the caller's job).
    pub match_exe: Option<String>,
    /// Optional window-title regex.
    pub match_title: Option<String>,
    /// 'email' | 'work_chat' | 'personal_chat' | 'code' | 'terminal' | 'docs' | 'other'
    pub category: String,
    /// 'default' | 'formal' | 'casual' | 'very_casual' | 'technical'
    pub style: String,
    /// 'none' | 'light' | 'medium' | 'high'; None = inherit global setting.
    pub cleanup_level: Option<String>,
    pub custom_prompt: Option<String>,
    /// 'auto' | 'paste' | 'paste_shift_insert' | 'type' | 'clipboard_only'
    pub insertion_method: String,
    /// 'raw' | 'shift_enter'
    pub newline_mode: String,
    /// Higher wins when several profiles match.
    pub priority: i64,
    /// Shipped with the app; not user-deletable.
    pub builtin: bool,
}

impl AppProfile {
    /// Minimal user profile; optional fields default per spec.
    pub fn new(name: &str, category: &str) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name: name.to_string(),
            match_exe: None,
            match_title: None,
            category: category.to_string(),
            style: "default".to_string(),
            cleanup_level: None,
            custom_prompt: None,
            insertion_method: "auto".to_string(),
            newline_mode: "raw".to_string(),
            priority: 0,
            builtin: false,
        }
    }

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            name: row.get("name")?,
            match_exe: row.get("match_exe")?,
            match_title: row.get("match_title")?,
            category: row.get("category")?,
            style: row.get("style")?,
            cleanup_level: row.get("cleanup_level")?,
            custom_prompt: row.get("custom_prompt")?,
            insertion_method: row.get("insertion_method")?,
            newline_mode: row.get("newline_mode")?,
            priority: row.get("priority")?,
            builtin: row.get("builtin")?,
        })
    }
}

pub trait AppProfileRepository {
    fn create(&self, profile: &AppProfile) -> Result<()>;
    fn get(&self, id: &str) -> Result<Option<AppProfile>>;
    /// All profiles ordered by `priority` DESC (match evaluation order).
    fn list(&self) -> Result<Vec<AppProfile>>;
    fn update(&self, profile: &AppProfile) -> Result<()>;
    fn delete(&self, id: &str) -> Result<()>;
}

pub struct SqliteAppProfileRepository<'a> {
    conn: &'a Connection,
}

impl<'a> SqliteAppProfileRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }
}

impl AppProfileRepository for SqliteAppProfileRepository<'_> {
    fn create(&self, profile: &AppProfile) -> Result<()> {
        self.conn.execute(
            "INSERT INTO app_profiles (
                id, name, match_exe, match_title, category, style,
                cleanup_level, custom_prompt, insertion_method,
                newline_mode, priority, builtin
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            params![
                profile.id,
                profile.name,
                profile.match_exe,
                profile.match_title,
                profile.category,
                profile.style,
                profile.cleanup_level,
                profile.custom_prompt,
                profile.insertion_method,
                profile.newline_mode,
                profile.priority,
                profile.builtin,
            ],
        )?;
        Ok(())
    }

    fn get(&self, id: &str) -> Result<Option<AppProfile>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM app_profiles WHERE id = ?1")?;
        let profile = stmt
            .query_row(params![id], AppProfile::from_row)
            .optional()?;
        Ok(profile)
    }

    fn list(&self) -> Result<Vec<AppProfile>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM app_profiles ORDER BY priority DESC, name ASC")?;
        let rows = stmt.query_map([], AppProfile::from_row)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    fn update(&self, profile: &AppProfile) -> Result<()> {
        self.conn.execute(
            "UPDATE app_profiles
             SET name = ?1, match_exe = ?2, match_title = ?3, category = ?4,
                 style = ?5, cleanup_level = ?6, custom_prompt = ?7,
                 insertion_method = ?8, newline_mode = ?9, priority = ?10
             WHERE id = ?11",
            params![
                profile.name,
                profile.match_exe,
                profile.match_title,
                profile.category,
                profile.style,
                profile.cleanup_level,
                profile.custom_prompt,
                profile.insertion_method,
                profile.newline_mode,
                profile.priority,
                profile.id,
            ],
        )?;
        Ok(())
    }

    fn delete(&self, id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM app_profiles WHERE id = ?1", params![id])?;
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
    fn crud_roundtrip_ordered_by_priority() {
        let conn = setup();
        let repo = SqliteAppProfileRepository::new(&conn);

        let mut slack = AppProfile::new("Slack", "work_chat");
        slack.match_exe = Some("slack.exe".to_string());
        slack.style = "casual".to_string();
        slack.priority = 10;

        let mut gmail = AppProfile::new("Gmail", "email");
        gmail.match_exe = Some("chrome.exe".to_string());
        gmail.match_title = Some("Gmail".to_string());
        gmail.style = "formal".to_string();
        gmail.priority = 20;
        gmail.builtin = true;

        repo.create(&slack).expect("create slack");
        repo.create(&gmail).expect("create gmail");

        let list = repo.list().expect("list");
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "Gmail"); // higher priority first

        let fetched = repo.get(&slack.id).expect("get").expect("exists");
        assert_eq!(fetched.insertion_method, "auto");
        assert!(fetched.cleanup_level.is_none());

        let mut edited = fetched.clone();
        edited.newline_mode = "shift_enter".to_string();
        edited.cleanup_level = Some("light".to_string());
        repo.update(&edited).expect("update");
        let fetched = repo.get(&slack.id).expect("get").expect("exists");
        assert_eq!(fetched.newline_mode, "shift_enter");
        assert_eq!(fetched.cleanup_level.as_deref(), Some("light"));

        repo.delete(&slack.id).expect("delete");
        assert_eq!(repo.list().expect("list").len(), 1);
    }

    #[test]
    fn invalid_category_is_rejected_by_check() {
        let conn = setup();
        let repo = SqliteAppProfileRepository::new(&conn);
        let p = AppProfile::new("X", "bogus");
        assert!(repo.create(&p).is_err());
    }
}
