//! Repository for `meeting_blocks` (migration 11): the durable clock
//! placement of every sealed audio block.
//!
//! `SealedBlock.start_offset_ms` exists only in memory during capture; these
//! rows persist it so post-processing (T-067) can place each
//! `<track>-NNNN.wav` on the meeting clock without guessing. The session
//! worker upserts one row per `BlockSealed` event and one per `CaptureSummary`
//! block at stop (the tail seals after the event channel is dropped), so a
//! crash mid-recording loses at most the block being written — exactly the
//! same guarantee the fsync'd files themselves give (AC-009-04).

use anyhow::Result;
use rusqlite::{params, Connection, Row};

/// Clock placement of one sealed block file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeetingBlock {
    pub meeting_id: String,
    /// `'mic' | 'system'` — matches `meeting_segments.track` values.
    pub track: String,
    /// 1-based index within the track (`mic-0001.wav` → 1).
    pub index: u32,
    /// Offset of the block's first frame from meeting start.
    pub start_ms: i64,
    pub duration_ms: i64,
}

impl MeetingBlock {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            meeting_id: row.get("meeting_id")?,
            track: row.get("track")?,
            index: row.get::<_, u32>("idx")?,
            start_ms: row.get("start_ms")?,
            duration_ms: row.get("duration_ms")?,
        })
    }
}

pub trait MeetingBlockRepository {
    /// Idempotent record of a sealed block's clock placement.
    fn upsert(&self, block: &MeetingBlock) -> Result<()>;
    /// Every recorded block of a meeting, ordered by track then index.
    fn list_by_meeting(&self, meeting_id: &str) -> Result<Vec<MeetingBlock>>;
}

pub struct SqliteMeetingBlockRepository<'a> {
    conn: &'a Connection,
}

impl<'a> SqliteMeetingBlockRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }
}

impl MeetingBlockRepository for SqliteMeetingBlockRepository<'_> {
    fn upsert(&self, block: &MeetingBlock) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO meeting_blocks
                (meeting_id, track, idx, start_ms, duration_ms)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                block.meeting_id,
                block.track,
                block.index,
                block.start_ms,
                block.duration_ms
            ],
        )?;
        Ok(())
    }

    fn list_by_meeting(&self, meeting_id: &str) -> Result<Vec<MeetingBlock>> {
        let mut stmt = self.conn.prepare(
            "SELECT meeting_id, track, idx, start_ms, duration_ms
             FROM meeting_blocks
             WHERE meeting_id = ?1
             ORDER BY track ASC, idx ASC",
        )?;
        let rows = stmt.query_map(params![meeting_id], MeetingBlock::from_row)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
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

    /// A persisted meeting — `meeting_blocks` FK-requires it.
    fn meeting(conn: &Connection) -> String {
        let m = Meeting::new("Meet", "manual");
        SqliteMeetingRepository::new(conn)
            .create(&m)
            .expect("create meeting");
        m.id
    }

    #[test]
    fn upsert_is_idempotent_and_lists_in_order() {
        let conn = setup();
        let meeting_id = meeting(&conn);
        let repo = SqliteMeetingBlockRepository::new(&conn);

        let block = MeetingBlock {
            meeting_id: meeting_id.clone(),
            track: "mic".into(),
            index: 1,
            start_ms: 0,
            duration_ms: 60_000,
        };
        repo.upsert(&block).expect("first upsert");
        // A pause/resume replaying the same index just refreshes the row.
        repo.upsert(&MeetingBlock {
            start_ms: 10,
            ..block.clone()
        })
        .expect("repeat upsert");

        repo.upsert(&MeetingBlock {
            meeting_id: meeting_id.clone(),
            track: "system".into(),
            index: 1,
            start_ms: 0,
            duration_ms: 60_000,
        })
        .expect("system block");

        let rows = repo.list_by_meeting(&meeting_id).expect("list");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].track, "mic");
        assert_eq!(rows[0].start_ms, 10);
        assert_eq!(rows[1].track, "system");
    }

    #[test]
    fn deleting_the_meeting_cascades_blocks() {
        let conn = setup();
        let meeting_id = meeting(&conn);
        let repo = SqliteMeetingBlockRepository::new(&conn);
        repo.upsert(&MeetingBlock {
            meeting_id: meeting_id.clone(),
            track: "mic".into(),
            index: 1,
            start_ms: 0,
            duration_ms: 60_000,
        })
        .expect("upsert");

        SqliteMeetingRepository::new(&conn)
            .delete(&meeting_id)
            .expect("delete meeting");
        assert!(repo.list_by_meeting(&meeting_id).expect("list").is_empty());
    }
}
