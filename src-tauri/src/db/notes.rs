//! Repository for `notes` (`data-model.md` §2): scratchpad, voice and meeting
//! notes. `meeting_id` cascades on meeting deletion.

use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension, Row};
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    /// uuid
    pub id: String,
    pub title: String,
    /// Markdown body.
    pub body_md: String,
    /// 'scratchpad' | 'voice' | 'meeting'
    pub source: String,
    pub meeting_id: Option<String>,
    pub pinned: bool,
    /// Unix epoch seconds.
    pub created_at: i64,
    pub updated_at: i64,
    pub exported_path: Option<String>,
}

impl Note {
    /// `source`: 'scratchpad' | 'voice' | 'meeting' ('meeting' notes should
    /// also pass `meeting_id`).
    pub fn new(source: &str) -> Self {
        let now = Utc::now().timestamp();
        Self {
            id: Uuid::new_v4().to_string(),
            title: String::new(),
            body_md: String::new(),
            source: source.to_string(),
            meeting_id: None,
            pinned: false,
            created_at: now,
            updated_at: now,
            exported_path: None,
        }
    }

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            title: row.get("title")?,
            body_md: row.get("body_md")?,
            source: row.get("source")?,
            meeting_id: row.get("meeting_id")?,
            pinned: row.get("pinned")?,
            created_at: row.get("created_at")?,
            updated_at: row.get("updated_at")?,
            exported_path: row.get("exported_path")?,
        })
    }
}

pub trait NoteRepository {
    fn create(&self, note: &Note) -> Result<()>;
    fn get(&self, id: &str) -> Result<Option<Note>>;
    /// Newest first; pinned notes come first.
    fn list(&self) -> Result<Vec<Note>>;
    fn list_by_meeting(&self, meeting_id: &str) -> Result<Vec<Note>>;
    /// Update editable fields; bumps `updated_at`.
    fn update(&self, note: &Note) -> Result<()>;
    fn delete(&self, id: &str) -> Result<()>;
}

pub struct SqliteNoteRepository<'a> {
    conn: &'a Connection,
}

impl<'a> SqliteNoteRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }
}

impl NoteRepository for SqliteNoteRepository<'_> {
    fn create(&self, note: &Note) -> Result<()> {
        self.conn.execute(
            "INSERT INTO notes (
                id, title, body_md, source, meeting_id, pinned,
                created_at, updated_at, exported_path
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                note.id,
                note.title,
                note.body_md,
                note.source,
                note.meeting_id,
                note.pinned,
                note.created_at,
                note.updated_at,
                note.exported_path,
            ],
        )?;
        Ok(())
    }

    fn get(&self, id: &str) -> Result<Option<Note>> {
        let mut stmt = self.conn.prepare("SELECT * FROM notes WHERE id = ?1")?;
        let note = stmt.query_row(params![id], Note::from_row).optional()?;
        Ok(note)
    }

    fn list(&self) -> Result<Vec<Note>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM notes ORDER BY pinned DESC, updated_at DESC")?;
        let rows = stmt.query_map([], Note::from_row)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    fn list_by_meeting(&self, meeting_id: &str) -> Result<Vec<Note>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM notes WHERE meeting_id = ?1 ORDER BY created_at ASC")?;
        let rows = stmt.query_map(params![meeting_id], Note::from_row)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    fn update(&self, note: &Note) -> Result<()> {
        self.conn.execute(
            "UPDATE notes
             SET title = ?1, body_md = ?2, pinned = ?3,
                 updated_at = ?4, exported_path = ?5
             WHERE id = ?6",
            params![
                note.title,
                note.body_md,
                note.pinned,
                Utc::now().timestamp(),
                note.exported_path,
                note.id,
            ],
        )?;
        Ok(())
    }

    fn delete(&self, id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM notes WHERE id = ?1", params![id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::meetings::{Meeting, MeetingRepository, SqliteMeetingRepository};
    use crate::db::run_migrations;

    fn setup() -> Connection {
        let mut conn = Connection::open_in_memory().expect("open in-memory db");
        crate::db::configure_connection(&conn).expect("configure");
        run_migrations(&mut conn).expect("run migrations");
        conn
    }

    #[test]
    fn crud_roundtrip() {
        let conn = setup();
        let notes = SqliteNoteRepository::new(&conn);

        let mut n = Note::new("scratchpad");
        n.title = "Ideias".to_string();
        n.body_md = "- item".to_string();
        notes.create(&n).expect("create");

        let fetched = notes.get(&n.id).expect("get").expect("exists");
        assert_eq!(fetched.title, "Ideias");
        assert!(!fetched.pinned);

        let mut edited = fetched;
        edited.pinned = true;
        edited.body_md = "- item\n- outro".to_string();
        notes.update(&edited).expect("update");
        let fetched = notes.get(&n.id).expect("get").expect("exists");
        assert!(fetched.pinned);
        assert!(fetched.updated_at >= fetched.created_at);

        notes.delete(&n.id).expect("delete");
        assert!(notes.get(&n.id).expect("get").is_none());
    }

    #[test]
    fn meeting_notes_cascade_with_meeting() {
        let conn = setup();
        let meetings = SqliteMeetingRepository::new(&conn);
        let notes = SqliteNoteRepository::new(&conn);

        let m = Meeting::new("Planning", "manual");
        meetings.create(&m).expect("create meeting");

        let mut n = Note::new("meeting");
        n.meeting_id = Some(m.id.clone());
        n.body_md = "minhas notas".to_string();
        notes.create(&n).expect("create note");

        assert_eq!(notes.list_by_meeting(&m.id).expect("list").len(), 1);

        meetings.delete(&m.id).expect("delete meeting");
        assert!(notes.list_by_meeting(&m.id).expect("list").is_empty());
    }

    #[test]
    fn invalid_source_is_rejected_by_check() {
        let conn = setup();
        let notes = SqliteNoteRepository::new(&conn);
        assert!(notes.create(&Note::new("bogus")).is_err());
    }
}
