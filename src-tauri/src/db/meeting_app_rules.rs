//! Repository for the `meeting_app_rules` table (`data-model.md` §2) —
//! FR-008-15 per-app detector rules (T-061). Split out of `meetings.rs`
//! to keep both files under the 800-line ratchet.

use anyhow::Result;
use rusqlite::{params, Connection, Row};
use serde::Serialize;
use specta::Type;
use uuid::Uuid;

/// Serialized+exported for the `meeting_rules_*` IPC commands (FR-008-15,
/// T-061) — the rules-settings screen binds straight to this shape.
#[derive(Clone, Debug, PartialEq, Serialize, Type)]
pub struct MeetingAppRule {
    /// uuid
    pub id: String,
    /// e.g. "Zoom.exe", "chrome.exe"
    pub exe: String,
    /// Optional window-title regex.
    pub title_pattern: Option<String>,
    /// e.g. "Zoom", "Google Meet"
    pub label: String,
    /// 'ask' | 'auto_start' | 'ignore'
    pub action: String,
    /// Shipped with the app; not user-deletable.
    pub builtin: bool,
}

impl MeetingAppRule {
    /// `action`: 'ask' | 'auto_start' | 'ignore'.
    pub fn new(exe: &str, label: &str, action: &str) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            exe: exe.to_string(),
            title_pattern: None,
            label: label.to_string(),
            action: action.to_string(),
            builtin: false,
        }
    }

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            exe: row.get("exe")?,
            title_pattern: row.get("title_pattern")?,
            label: row.get("label")?,
            action: row.get("action")?,
            builtin: row.get("builtin")?,
        })
    }
}

pub trait MeetingAppRuleRepository {
    fn create(&self, rule: &MeetingAppRule) -> Result<()>;
    /// Rules whose `exe` matches (exact, case-insensitive).
    fn find_by_exe(&self, exe: &str) -> Result<Vec<MeetingAppRule>>;
    fn list(&self) -> Result<Vec<MeetingAppRule>>;
    /// Update only the `action` column ('ask' | 'auto_start' | 'ignore';
    /// FR-008-15). No-op when `id` does not exist.
    fn set_action(&self, id: &str, action: &str) -> Result<()>;
    fn delete(&self, id: &str) -> Result<()>;
}

pub struct SqliteMeetingAppRuleRepository<'a> {
    conn: &'a Connection,
}

impl<'a> SqliteMeetingAppRuleRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }
}

impl MeetingAppRuleRepository for SqliteMeetingAppRuleRepository<'_> {
    fn create(&self, rule: &MeetingAppRule) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meeting_app_rules (id, exe, title_pattern, label, action, builtin)
             VALUES (?1,?2,?3,?4,?5,?6)",
            params![
                rule.id,
                rule.exe,
                rule.title_pattern,
                rule.label,
                rule.action,
                rule.builtin
            ],
        )?;
        Ok(())
    }

    fn find_by_exe(&self, exe: &str) -> Result<Vec<MeetingAppRule>> {
        let mut stmt = self.conn.prepare(
            "SELECT * FROM meeting_app_rules WHERE exe = ?1 COLLATE NOCASE ORDER BY label ASC",
        )?;
        let rows = stmt.query_map(params![exe], MeetingAppRule::from_row)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    fn list(&self) -> Result<Vec<MeetingAppRule>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM meeting_app_rules ORDER BY label ASC")?;
        let rows = stmt.query_map([], MeetingAppRule::from_row)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    fn set_action(&self, id: &str, action: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE meeting_app_rules SET action = ?1 WHERE id = ?2",
            params![action, id],
        )?;
        Ok(())
    }

    fn delete(&self, id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM meeting_app_rules WHERE id = ?1", params![id])?;
        Ok(())
    }
}
