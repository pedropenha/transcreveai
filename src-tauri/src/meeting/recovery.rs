//! Crash recovery for meetings (FR-009-05, AC-009-04).
//!
//! A meeting that was `recording` or `paused` when the process died is an
//! orphan: the app enforces a single instance, so any meeting still in an
//! in-progress status at startup has no live capture behind it. Such meetings
//! are moved to `recovered` so the session layer (T-064) can offer them for
//! processing; their audio is read back with
//! [`super::blocks::scan_meeting_blocks`]. A `processing` row is likewise
//! orphaned — queued post-processing jobs do not survive a restart — and is
//! moved to `error` so it remains retriable instead of spinning forever.

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::Connection;

use crate::db::meetings::{Meeting, MeetingRepository, SqliteMeetingRepository};

/// Statuses that lose their in-process owner when the app exits. `paused`
/// counts as capture-owned; `processing` is owned by the post-processing
/// worker and must not survive a restart as a permanent spinner.
const INTERRUPTED_STATUSES: [&str; 3] = ["recording", "paused", "processing"];

/// Pure status classifier, kept free of the DB so the policy is testable.
pub(crate) fn is_interrupted_status(status: &str) -> bool {
    INTERRUPTED_STATUSES.contains(&status)
}

fn recovery_status_for(status: &str) -> Option<(&'static str, &'static str)> {
    match status {
        "recording" | "paused" => Some(("recovered", "interrupted")),
        "processing" => Some(("error", "processing_interrupted")),
        _ => None,
    }
}

/// Mark every orphaned in-progress meeting `recovered` (capture interrupted)
/// or `error` (post-processing interrupted). Returns the affected meetings
/// (post-update) so the caller can log or surface them.
///
/// Idempotent: a second run finds nothing to recover.
pub fn recover_interrupted_meetings(conn: &Connection) -> Result<Vec<Meeting>> {
    let repo = SqliteMeetingRepository::new(conn);
    let interrupted: Vec<Meeting> = repo
        .list()
        .context("Failed to list meetings for recovery")?
        .into_iter()
        .filter(|meeting| is_interrupted_status(&meeting.status))
        .collect();

    let mut recovered = Vec::with_capacity(interrupted.len());
    for meeting in interrupted {
        let Some((status, code)) = recovery_status_for(&meeting.status) else {
            continue;
        };
        repo.set_status(&meeting.id, status, Some(code))
            .with_context(|| format!("Failed to mark meeting {} as {status}", meeting.id))?;
        if let Some(updated) = repo
            .get(&meeting.id)
            .with_context(|| format!("Failed to reload meeting {}", meeting.id))?
        {
            recovered.push(updated);
        }
    }
    Ok(recovered)
}

/// Resolve the database under `app_data_dir`, open it, and run
/// [`recover_interrupted_meetings`]. Called once at startup — migrations must
/// already have run (the history manager does that first).
pub fn recover_interrupted_meetings_at(app_data_dir: &Path) -> Result<Vec<Meeting>> {
    let db_path = crate::db::database_path(app_data_dir)?;
    let conn = crate::db::open_connection(&db_path)
        .with_context(|| format!("Failed to open {} for meeting recovery", db_path.display()))?;
    recover_interrupted_meetings(&conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::meetings::SqliteMeetingRepository;
    use crate::db::run_migrations;

    fn setup() -> Connection {
        let mut conn = Connection::open_in_memory().expect("open in-memory db");
        crate::db::configure_connection(&conn).expect("configure");
        run_migrations(&mut conn).expect("run migrations");
        conn
    }

    fn meeting_with_status(conn: &Connection, status: &str) -> Meeting {
        let repo = SqliteMeetingRepository::new(conn);
        let mut meeting = Meeting::new("call", "auto_prompt");
        meeting.status = status.to_string();
        repo.create(&meeting).expect("create meeting");
        meeting
    }

    #[test]
    fn recording_and_paused_meetings_are_recovered() {
        let conn = setup();
        let repo = SqliteMeetingRepository::new(&conn);

        let recording = meeting_with_status(&conn, "recording");
        let paused = meeting_with_status(&conn, "paused");
        let processing = meeting_with_status(&conn, "processing");
        let ready = meeting_with_status(&conn, "ready");
        let already = meeting_with_status(&conn, "recovered");

        let recovered = recover_interrupted_meetings(&conn).expect("recover");
        let mut ids: Vec<&str> = recovered.iter().map(|m| m.id.as_str()).collect();
        ids.sort();
        let mut expected = vec![
            recording.id.as_str(),
            paused.id.as_str(),
            processing.id.as_str(),
        ];
        expected.sort();
        assert_eq!(ids, expected);

        for id in [recording.id.as_str(), paused.id.as_str()] {
            let meeting = repo.get(id).expect("get").expect("exists");
            assert_eq!(meeting.status, "recovered");
            assert_eq!(meeting.error_code.as_deref(), Some("interrupted"));
            // Recovery stamps ended_at so the meeting has a real end time.
            assert!(meeting.ended_at.is_some());
        }
        let interrupted_processing = repo.get(&processing.id).unwrap().unwrap();
        assert_eq!(interrupted_processing.status, "error");
        assert_eq!(
            interrupted_processing.error_code.as_deref(),
            Some("processing_interrupted")
        );
        assert!(interrupted_processing.ended_at.is_some());
        // Terminal/already-recovered meetings are untouched.
        assert_eq!(repo.get(&ready.id).unwrap().unwrap().status, "ready");
        assert_eq!(repo.get(&already.id).unwrap().unwrap().error_code, None);

        // Idempotent: nothing left to recover.
        assert!(recover_interrupted_meetings(&conn).unwrap().is_empty());
    }
}
