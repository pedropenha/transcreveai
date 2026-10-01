//! Repository for `meeting_blocks` (migration 11, T-065): the persisted
//! pending queue for transcription bookkeeping, sealed-block by sealed-block.

use anyhow::Result;
use rusqlite::{params, Connection};

// ---------------------------------------------------------------------------
// meeting_blocks
// ---------------------------------------------------------------------------

/// Transcription bookkeeping for one sealed audio block (T-065; the pending
/// handoff to T-067). The session worker inserts a row the moment a block
/// seals (`transcribed = 0`); the live transcriber flips `transcribed` once
/// every utterance in the block was attempted. A row left at
/// `transcribed = 0` when the meeting stops — queued, failed, or still in
/// flight — is exactly what post-processing still owes.
///
/// The WAV path is derived, not stored: `meetings.audio_dir` +
/// `meeting::blocks::block_filename(track, block_index)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeetingBlock {
    pub meeting_id: String,
    /// 'mic' | 'system'
    pub track: String,
    /// 1-based index within the track (`mic-0001.wav` carries index 1).
    pub block_index: i64,
    /// Offsets on the meeting clock — `SealedBlock::start_offset_ms` +
    /// `duration_ms`.
    pub start_ms: i64,
    pub end_ms: i64,
    pub transcribed: bool,
    /// Completed processing passes that did not fully succeed.
    pub attempts: i64,
}

pub trait MeetingBlockRepository {
    /// First sighting of a sealed block — `INSERT OR IGNORE` so a re-push or
    /// a pause/resume replay never resets an existing row's progress.
    fn record_pending(&self, block: &MeetingBlock) -> Result<()>;
    /// Every utterance of the block was processed.
    fn mark_transcribed(&self, meeting_id: &str, track: &str, block_index: i64) -> Result<()>;
    /// The block's pass failed (STT error, unreadable WAV, …): it stays
    /// pending and the attempt counter records the effort.
    fn record_failure(&self, meeting_id: &str, track: &str, block_index: i64) -> Result<()>;
    /// Blocks still owed transcription — oldest offsets first. T-067's
    /// processing queue.
    fn list_pending(&self, meeting_id: &str) -> Result<Vec<MeetingBlock>>;
    /// Every recorded block of a meeting — transcribed or not — ordered by
    /// track then index. T-067's clock-placement source for coverage.
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
    fn record_pending(&self, block: &MeetingBlock) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO meeting_blocks (
                meeting_id, track, block_index, start_ms, end_ms, transcribed, attempts
            ) VALUES (?1,?2,?3,?4,?5,0,0)",
            params![
                block.meeting_id,
                block.track,
                block.block_index,
                block.start_ms,
                block.end_ms,
            ],
        )?;
        Ok(())
    }

    fn mark_transcribed(&self, meeting_id: &str, track: &str, block_index: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE meeting_blocks SET transcribed = 1
             WHERE meeting_id = ?1 AND track = ?2 AND block_index = ?3",
            params![meeting_id, track, block_index],
        )?;
        Ok(())
    }

    fn record_failure(&self, meeting_id: &str, track: &str, block_index: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE meeting_blocks SET attempts = attempts + 1
             WHERE meeting_id = ?1 AND track = ?2 AND block_index = ?3",
            params![meeting_id, track, block_index],
        )?;
        Ok(())
    }

    fn list_pending(&self, meeting_id: &str) -> Result<Vec<MeetingBlock>> {
        let mut stmt = self.conn.prepare(
            "SELECT meeting_id, track, block_index, start_ms, end_ms, transcribed, attempts
             FROM meeting_blocks
             WHERE meeting_id = ?1 AND transcribed = 0
             ORDER BY start_ms ASC, track ASC, block_index ASC",
        )?;
        let rows = stmt.query_map(params![meeting_id], |row| {
            Ok(MeetingBlock {
                meeting_id: row.get("meeting_id")?,
                track: row.get("track")?,
                block_index: row.get("block_index")?,
                start_ms: row.get("start_ms")?,
                end_ms: row.get("end_ms")?,
                transcribed: row.get("transcribed")?,
                attempts: row.get("attempts")?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    fn list_by_meeting(&self, meeting_id: &str) -> Result<Vec<MeetingBlock>> {
        let mut stmt = self.conn.prepare(
            "SELECT meeting_id, track, block_index, start_ms, end_ms, transcribed, attempts
             FROM meeting_blocks
             WHERE meeting_id = ?1
             ORDER BY track ASC, block_index ASC",
        )?;
        let rows = stmt.query_map(params![meeting_id], |row| {
            Ok(MeetingBlock {
                meeting_id: row.get("meeting_id")?,
                track: row.get("track")?,
                block_index: row.get("block_index")?,
                start_ms: row.get("start_ms")?,
                end_ms: row.get("end_ms")?,
                transcribed: row.get("transcribed")?,
                attempts: row.get("attempts")?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::meetings::{Meeting, MeetingRepository, SqliteMeetingRepository};

    /// Migrated in-memory db with one meeting row to key the blocks against.
    /// `configure_connection` turns `foreign_keys` on — the cascade test
    /// depends on it.
    fn setup() -> (Connection, String) {
        let mut conn = Connection::open_in_memory().expect("open in-memory db");
        crate::db::configure_connection(&conn).expect("configure connection");
        crate::db::run_migrations(&mut conn).expect("run migrations");
        let meeting = Meeting::new("M · teste", "manual");
        SqliteMeetingRepository::new(&conn)
            .create(&meeting)
            .expect("insert meeting");
        (conn, meeting.id)
    }

    fn pending(meeting_id: &str, track: &str, index: i64, start_ms: i64) -> MeetingBlock {
        MeetingBlock {
            meeting_id: meeting_id.to_string(),
            track: track.to_string(),
            block_index: index,
            start_ms,
            end_ms: start_ms + 60_000,
            transcribed: false,
            attempts: 0,
        }
    }

    #[test]
    fn record_pending_is_idempotent_and_pending_lists_oldest_first() {
        let (conn, id) = setup();
        let repo = SqliteMeetingBlockRepository::new(&conn);
        repo.record_pending(&pending(&id, "mic", 1, 0)).unwrap();
        repo.record_pending(&pending(&id, "mic", 2, 60_000))
            .unwrap();
        repo.record_pending(&pending(&id, "system", 1, 0)).unwrap();
        // Replay (e.g. pause/resume re-seal) keeps the row, not a reset.
        repo.record_pending(&pending(&id, "mic", 1, 0)).unwrap();

        let rows = repo.list_pending(&id).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(
            rows.iter()
                .map(|r| (r.track.as_str(), r.block_index))
                .collect::<Vec<_>>(),
            vec![("mic", 1), ("system", 1), ("mic", 2)]
        );
        assert!(rows.iter().all(|r| !r.transcribed && r.attempts == 0));
    }

    #[test]
    fn mark_transcribed_leaves_pending_and_failure_counts_attempts() {
        let (conn, id) = setup();
        let repo = SqliteMeetingBlockRepository::new(&conn);
        repo.record_pending(&pending(&id, "mic", 1, 0)).unwrap();
        repo.record_pending(&pending(&id, "mic", 2, 60_000))
            .unwrap();

        repo.mark_transcribed(&id, "mic", 1).unwrap();
        repo.record_failure(&id, "mic", 2).unwrap();

        let rows = repo.list_pending(&id).unwrap();
        assert_eq!(rows.len(), 1, "transcribed blocks leave the queue");
        assert_eq!(rows[0].block_index, 2);
        assert_eq!(rows[0].attempts, 1);

        // A failed block that later succeeds leaves the pending list.
        repo.mark_transcribed(&id, "mic", 2).unwrap();
        assert!(repo.list_pending(&id).unwrap().is_empty());
    }

    #[test]
    fn blocks_cascade_with_the_meeting() {
        let (conn, id) = setup();
        SqliteMeetingBlockRepository::new(&conn)
            .record_pending(&pending(&id, "mic", 1, 0))
            .unwrap();
        SqliteMeetingRepository::new(&conn).delete(&id).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM meeting_blocks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "ON DELETE CASCADE removes the bookkeeping");
    }
}
