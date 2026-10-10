//! Repositories for the meetings domain (`data-model.md` §2): `meetings`,
//! `meeting_segments` (app rules live in `meeting_app_rules.rs`).

use anyhow::Result;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;
use specta::Type;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// meetings
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Type)]
pub struct Meeting {
    /// uuid
    pub id: String,
    pub title: String,
    /// e.g. "Zoom.exe", "chrome.exe"
    pub app_exe: Option<String>,
    /// e.g. "Google Meet"
    pub app_label: Option<String>,
    /// Full path of the detected executable (migration 15) — only used to
    /// extract the source-app icon; never sent over IPC (`serde(skip)`).
    /// Always passed through `is_safe_exe_path` before being stored.
    #[serde(skip)]
    pub app_exe_path: Option<String>,
    /// 'auto_prompt' | 'auto_start' | 'manual' | 'in_person'
    pub detection: String,
    /// 'recording' | 'paused' | 'processing' | 'ready' | 'error' | 'recovered'
    pub status: String,
    /// Unix epoch seconds.
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub capture_system_audio: bool,
    pub stt_provider_id: Option<String>,
    pub llm_provider_id: Option<String>,
    pub template_id: Option<String>,
    /// Editable markdown summary.
    pub summary_md: Option<String>,
    /// 'pending' | 'ready' | 'disabled' | 'error' — FR-009-16 step (4).
    /// 'disabled' is FR-009-21 (no BYOK key): the meeting is `ready` with
    /// transcript + notes but no summary.
    pub summary_status: String,
    /// Directory with the 60 s audio blocks; NULL after retention expiry.
    pub audio_dir: Option<String>,
    pub language: Option<String>,
    pub error_code: Option<String>,
}

impl Meeting {
    /// A meeting detected/started now with the given `detection` trigger.
    /// `detection`: 'auto_prompt' | 'auto_start' | 'manual' | 'in_person'.
    pub fn new(title: &str, detection: &str) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            title: title.to_string(),
            app_exe: None,
            app_label: None,
            app_exe_path: None,
            detection: detection.to_string(),
            status: "recording".to_string(),
            started_at: Utc::now().timestamp(),
            ended_at: None,
            capture_system_audio: true,
            stt_provider_id: None,
            llm_provider_id: None,
            template_id: None,
            summary_md: None,
            summary_status: "pending".to_string(),
            audio_dir: None,
            language: None,
            error_code: None,
        }
    }

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            title: row.get("title")?,
            app_exe: row.get("app_exe")?,
            app_label: row.get("app_label")?,
            app_exe_path: row.get("app_exe_path")?,
            detection: row.get("detection")?,
            status: row.get("status")?,
            started_at: row.get("started_at")?,
            ended_at: row.get("ended_at")?,
            capture_system_audio: row.get("capture_system_audio")?,
            stt_provider_id: row.get("stt_provider_id")?,
            llm_provider_id: row.get("llm_provider_id")?,
            template_id: row.get("template_id")?,
            summary_md: row.get("summary_md")?,
            summary_status: row.get("summary_status")?,
            audio_dir: row.get("audio_dir")?,
            language: row.get("language")?,
            error_code: row.get("error_code")?,
        })
    }
}

pub trait MeetingRepository {
    fn create(&self, meeting: &Meeting) -> Result<()>;
    fn get(&self, id: &str) -> Result<Option<Meeting>>;
    /// Newest first (by `started_at`).
    fn list(&self) -> Result<Vec<Meeting>>;
    /// Full-text search over title, summary, "Minhas notas" and transcript
    /// (FR-009-25). `fts_query` is a sanitized FTS5 expression — build it
    /// with `crate::db::fts_match_query`, never from raw user input.
    fn search(&self, fts_query: &str) -> Result<Vec<Meeting>>;
    /// Persist every mutable field of `meeting`.
    fn update(&self, meeting: &Meeting) -> Result<()>;
    /// Set `status` (and optionally `error_code`); when `ended` is true also
    /// stamps `ended_at` if not already set.
    fn set_status(&self, id: &str, status: &str, error_code: Option<&str>) -> Result<()>;
    /// Deletes the meeting and cascades to its segments/notes.
    fn delete(&self, id: &str) -> Result<()>;
    /// T-067: persist the generated/edited summary together with its status
    /// ('ready' | 'disabled' | 'error'). Never touches `notes` (FR-009-19).
    fn set_summary(&self, id: &str, summary_md: Option<&str>, status: &str) -> Result<()>;
    /// T-067: flip `summary_status` only (e.g. back to 'pending' while a
    /// regeneration is in flight).
    fn set_summary_status(&self, id: &str, status: &str) -> Result<()>;
    /// T-067: per-meeting summary-template override; `None` falls back to the
    /// `summary_templates.is_default` row.
    fn set_template_id(&self, id: &str, template_id: Option<&str>) -> Result<()>;
    /// T-067: write a title — callers gate on "still the default placeholder"
    /// so a user-edited title is never clobbered.
    fn set_title(&self, id: &str, title: &str) -> Result<()>;
    /// T-115: clear `audio_dir` after an audio-only delete removed the
    /// blocks — the row keeps transcript, notes and summary.
    fn clear_audio_dir(&self, id: &str) -> Result<()>;
}

pub struct SqliteMeetingRepository<'a> {
    conn: &'a Connection,
}

impl<'a> SqliteMeetingRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }
}

impl MeetingRepository for SqliteMeetingRepository<'_> {
    fn create(&self, meeting: &Meeting) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meetings (
                id, title, app_exe, app_label, detection, status,
                started_at, ended_at, capture_system_audio,
                stt_provider_id, llm_provider_id, template_id,
                summary_md, summary_status, audio_dir, language, error_code,
                app_exe_path
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)",
            params![
                meeting.id,
                meeting.title,
                meeting.app_exe,
                meeting.app_label,
                meeting.detection,
                meeting.status,
                meeting.started_at,
                meeting.ended_at,
                meeting.capture_system_audio,
                meeting.stt_provider_id,
                meeting.llm_provider_id,
                meeting.template_id,
                meeting.summary_md,
                meeting.summary_status,
                meeting.audio_dir,
                meeting.language,
                meeting.error_code,
                meeting.app_exe_path,
            ],
        )?;
        Ok(())
    }

    fn get(&self, id: &str) -> Result<Option<Meeting>> {
        let mut stmt = self.conn.prepare("SELECT * FROM meetings WHERE id = ?1")?;
        let meeting = stmt.query_row(params![id], Meeting::from_row).optional()?;
        Ok(meeting)
    }

    fn list(&self) -> Result<Vec<Meeting>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM meetings ORDER BY started_at DESC")?;
        let rows = stmt.query_map([], Meeting::from_row)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Unions the three FTS sources: `meetings_fts` (title + summary),
    /// `notes_fts` (meeting notes rows) and `meeting_fts` (segment text).
    /// `meetings` has a TEXT uuid primary key, so the FTS joins go through
    /// the implicit `rowid` the external-content tables were created with.
    fn search(&self, fts_query: &str) -> Result<Vec<Meeting>> {
        let mut stmt = self.conn.prepare(
            "SELECT m.* FROM meetings m
             WHERE EXISTS (
                     SELECT 1 FROM meetings_fts
                     WHERE meetings_fts MATCH ?1 AND meetings_fts.rowid = m.rowid
                   )
                OR EXISTS (
                     SELECT 1 FROM notes_fts
                     JOIN notes n ON n.rowid = notes_fts.rowid
                     WHERE notes_fts MATCH ?1 AND n.meeting_id = m.id
                   )
                OR EXISTS (
                     SELECT 1 FROM meeting_fts
                     JOIN meeting_segments s ON s.rowid = meeting_fts.rowid
                     WHERE meeting_fts MATCH ?1 AND s.meeting_id = m.id
                       AND s.excluded = 0
                   )
             ORDER BY m.started_at DESC",
        )?;
        let rows = stmt.query_map(params![fts_query], Meeting::from_row)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    fn update(&self, meeting: &Meeting) -> Result<()> {
        self.conn.execute(
            "UPDATE meetings SET
                title = ?1, app_exe = ?2, app_label = ?3, detection = ?4,
                status = ?5, started_at = ?6, ended_at = ?7,
                capture_system_audio = ?8, stt_provider_id = ?9,
                llm_provider_id = ?10, template_id = ?11, summary_md = ?12,
                summary_status = ?13, audio_dir = ?14, language = ?15,
                error_code = ?16, app_exe_path = ?17
             WHERE id = ?18",
            params![
                meeting.title,
                meeting.app_exe,
                meeting.app_label,
                meeting.detection,
                meeting.status,
                meeting.started_at,
                meeting.ended_at,
                meeting.capture_system_audio,
                meeting.stt_provider_id,
                meeting.llm_provider_id,
                meeting.template_id,
                meeting.summary_md,
                meeting.summary_status,
                meeting.audio_dir,
                meeting.language,
                meeting.error_code,
                meeting.app_exe_path,
                meeting.id,
            ],
        )?;
        Ok(())
    }

    fn set_status(&self, id: &str, status: &str, error_code: Option<&str>) -> Result<()> {
        let ended = matches!(status, "ready" | "error" | "recovered");
        self.conn.execute(
            "UPDATE meetings SET
                status = ?1,
                error_code = ?2,
                ended_at = CASE WHEN ?3 AND ended_at IS NULL THEN ?4 ELSE ended_at END
             WHERE id = ?5",
            params![status, error_code, ended, Utc::now().timestamp(), id],
        )?;
        Ok(())
    }

    fn delete(&self, id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM meetings WHERE id = ?1", params![id])?;
        Ok(())
    }

    fn set_summary(&self, id: &str, summary_md: Option<&str>, status: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE meetings SET summary_md = ?1, summary_status = ?2 WHERE id = ?3",
            params![summary_md, status, id],
        )?;
        Ok(())
    }

    fn set_summary_status(&self, id: &str, status: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE meetings SET summary_status = ?1 WHERE id = ?2",
            params![status, id],
        )?;
        Ok(())
    }

    fn set_template_id(&self, id: &str, template_id: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE meetings SET template_id = ?1 WHERE id = ?2",
            params![template_id, id],
        )?;
        Ok(())
    }

    fn set_title(&self, id: &str, title: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE meetings SET title = ?1 WHERE id = ?2",
            params![title, id],
        )?;
        Ok(())
    }

    fn clear_audio_dir(&self, id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE meetings SET audio_dir = NULL WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// meeting_segments
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Type)]
pub struct MeetingSegment {
    /// uuid
    pub id: String,
    pub meeting_id: String,
    /// 'mic' | 'system'
    pub track: String,
    /// "Você", "Outros", "Falante 1"…
    pub speaker: Option<String>,
    /// Milliseconds relative to the meeting's `started_at`.
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
    /// 'speech' | 'dictation_marker' | 'gap_marker'
    pub kind: String,
    /// false while the segment is still a partial transcript.
    pub is_final: bool,
    /// FR-009-10/AC-009-03 (T-065): mic speech overlapping a dictation
    /// interval is excluded from the transcript — the meeting UI hides these
    /// rows and shows the covering `dictation_marker` instead. Never set on
    /// `system` rows or on markers themselves.
    pub excluded: bool,
}

impl MeetingSegment {
    /// `track`: 'mic' | 'system'. `kind` defaults to 'speech'.
    pub fn new(meeting_id: &str, track: &str, start_ms: i64, end_ms: i64, text: &str) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            meeting_id: meeting_id.to_string(),
            track: track.to_string(),
            speaker: None,
            start_ms,
            end_ms,
            text: text.to_string(),
            kind: "speech".to_string(),
            is_final: true,
            excluded: false,
        }
    }

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            meeting_id: row.get("meeting_id")?,
            track: row.get("track")?,
            speaker: row.get("speaker")?,
            start_ms: row.get("start_ms")?,
            end_ms: row.get("end_ms")?,
            text: row.get("text")?,
            kind: row.get("kind")?,
            is_final: row.get("is_final")?,
            excluded: row.get("excluded")?,
        })
    }
}

pub trait MeetingSegmentRepository {
    fn create(&self, segment: &MeetingSegment) -> Result<()>;
    /// Segments of a meeting ordered by `start_ms`.
    fn list_by_meeting(&self, meeting_id: &str) -> Result<Vec<MeetingSegment>>;
    /// FR-009-10: flag `kind = 'speech'` rows on `track` overlapping the
    /// `[start_ms, end_ms)` dictation interval. Runs when the interval
    /// closes so segments already persisted are excluded too (the live
    /// transcriber also flags at insert — this is the catch-up half).
    /// Returns how many rows were marked.
    fn mark_speech_excluded(
        &self,
        meeting_id: &str,
        track: &str,
        start_ms: i64,
        end_ms: i64,
    ) -> Result<usize>;
    fn delete(&self, id: &str) -> Result<()>;
}

pub struct SqliteMeetingSegmentRepository<'a> {
    conn: &'a Connection,
}

impl<'a> SqliteMeetingSegmentRepository<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }
}

impl MeetingSegmentRepository for SqliteMeetingSegmentRepository<'_> {
    fn create(&self, segment: &MeetingSegment) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meeting_segments (
                id, meeting_id, track, speaker, start_ms, end_ms, text, kind,
                is_final, excluded
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                segment.id,
                segment.meeting_id,
                segment.track,
                segment.speaker,
                segment.start_ms,
                segment.end_ms,
                segment.text,
                segment.kind,
                segment.is_final,
                segment.excluded,
            ],
        )?;
        Ok(())
    }

    fn list_by_meeting(&self, meeting_id: &str) -> Result<Vec<MeetingSegment>> {
        let mut stmt = self.conn.prepare(
            "SELECT * FROM meeting_segments WHERE meeting_id = ?1 ORDER BY start_ms ASC",
        )?;
        let rows = stmt.query_map(params![meeting_id], MeetingSegment::from_row)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    fn mark_speech_excluded(
        &self,
        meeting_id: &str,
        track: &str,
        start_ms: i64,
        end_ms: i64,
    ) -> Result<usize> {
        let marked = self.conn.execute(
            "UPDATE meeting_segments SET excluded = 1
             WHERE meeting_id = ?1 AND track = ?2 AND kind = 'speech'
               AND start_ms < ?3 AND end_ms > ?4",
            params![meeting_id, track, end_ms, start_ms],
        )?;
        Ok(marked)
    }

    fn delete(&self, id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM meeting_segments WHERE id = ?1", params![id])?;
        Ok(())
    }
}

// `meeting_blocks` (T-065 live-transcription bookkeeping) lives in
// `db/meeting_blocks.rs` — its own table, its own file.

// `meeting_app_rules` lives in `db/meeting_app_rules.rs` — split to
// keep this file under the 800-line ratchet. Re-exported so existing
// `db::meetings::MeetingAppRule` paths keep working.
pub use super::meeting_app_rules::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::run_migrations;

    fn setup() -> Connection {
        let mut conn = Connection::open_in_memory().expect("open in-memory db");
        crate::db::configure_connection(&conn).expect("configure");
        run_migrations(&mut conn).expect("run migrations");
        conn
    }

    #[test]
    fn meeting_lifecycle_and_segments() {
        let conn = setup();
        let meetings = SqliteMeetingRepository::new(&conn);
        let segments = SqliteMeetingSegmentRepository::new(&conn);

        let mut m = Meeting::new("Daily", "auto_prompt");
        m.app_exe = Some("zoom.exe".to_string());
        m.app_label = Some("Zoom".to_string());
        meetings.create(&m).expect("create meeting");

        let mut s1 = MeetingSegment::new(&m.id, "mic", 0, 1500, "bom dia");
        s1.speaker = Some("Você".to_string());
        let s2 = MeetingSegment::new(&m.id, "system", 1500, 3000, "oi pessoal");
        segments.create(&s2).expect("seg2");
        segments.create(&s1).expect("seg1");

        let segs = segments.list_by_meeting(&m.id).expect("list segments");
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].text, "bom dia"); // ordered by start_ms

        meetings
            .set_status(&m.id, "ready", None)
            .expect("set status");
        let fetched = meetings.get(&m.id).expect("get").expect("exists");
        assert_eq!(fetched.status, "ready");
        assert!(fetched.ended_at.is_some());

        // Full update path.
        let mut edited = fetched;
        edited.summary_md = Some("# Resumo".to_string());
        meetings.update(&edited).expect("update");
        assert_eq!(
            meetings
                .get(&m.id)
                .expect("get")
                .expect("exists")
                .summary_md,
            Some("# Resumo".to_string())
        );

        // Deleting the meeting cascades to its segments (FK ON).
        meetings.delete(&m.id).expect("delete");
        assert!(segments.list_by_meeting(&m.id).expect("list").is_empty());
    }

    #[test]
    fn search_matches_title_summary_notes_and_transcript() {
        // FR-009-25 (T-068): full-text over title, summary, notes and
        // transcript via the three FTS tables unioned by `search`.
        use crate::db::notes::{Note, NoteRepository, SqliteNoteRepository};

        let conn = setup();
        let meetings = SqliteMeetingRepository::new(&conn);
        let segments = SqliteMeetingSegmentRepository::new(&conn);
        let notes = SqliteNoteRepository::new(&conn);
        let q = |raw: &str| crate::db::fts_match_query(raw).expect("non-empty query");

        // Four meetings, each matching a different source.
        let mut by_title = Meeting::new("Retro kubernetes", "manual");
        by_title.started_at = 100;
        meetings.create(&by_title).expect("create by_title");

        let mut by_summary = Meeting::new("Planning", "manual");
        by_summary.started_at = 200;
        by_summary.summary_md = Some("decidimos migrar para oauth".to_string());
        meetings.create(&by_summary).expect("create by_summary");

        let mut by_notes = Meeting::new("Daily", "manual");
        by_notes.started_at = 300;
        meetings.create(&by_notes).expect("create by_notes");
        let mut n = Note::new("meeting");
        n.meeting_id = Some(by_notes.id.clone());
        n.body_md = "cliente pediu desconto".to_string();
        notes.create(&n).expect("create note");

        let mut by_segment = Meeting::new("1:1", "manual");
        by_segment.started_at = 400;
        meetings.create(&by_segment).expect("create by_segment");
        segments
            .create(&MeetingSegment::new(
                &by_segment.id,
                "mic",
                0,
                1500,
                "okr review",
            ))
            .expect("create segment");

        let mut noise = Meeting::new("Weekly sync", "manual");
        noise.started_at = 500;
        meetings.create(&noise).expect("create noise");

        let ids = |term: &str| -> Vec<String> {
            meetings
                .search(&q(term))
                .expect("search")
                .into_iter()
                .map(|m| m.id)
                .collect()
        };

        assert_eq!(ids("kubernetes"), vec![by_title.id.clone()]);
        assert_eq!(ids("oauth"), vec![by_summary.id.clone()]);
        assert_eq!(ids("desconto"), vec![by_notes.id]);
        assert_eq!(ids("okr"), vec![by_segment.id.clone()]);
        assert!(ids("inexistente").is_empty());

        // Prefix matching + edited summary stays searchable.
        assert_eq!(ids("kube"), vec![by_title.id]);
        by_summary.summary_md = Some("rollback plan".to_string());
        meetings.update(&by_summary).expect("update summary");
        assert!(ids("oauth").is_empty());
        assert_eq!(ids("rollback"), vec![by_summary.id]);

        // Deleting the meeting removes every FTS trace of it.
        meetings.delete(&by_segment.id).expect("delete");
        assert!(ids("okr").is_empty());
    }

    #[test]
    fn search_never_matches_excluded_dictated_speech() {
        // FR-009-10/AC-009-03: dictated mic rows are private — neither the
        // insert trigger nor the search join may surface them.
        let conn = setup();
        let meetings = SqliteMeetingRepository::new(&conn);
        let segments = SqliteMeetingSegmentRepository::new(&conn);
        let q = |raw: &str| crate::db::fts_match_query(raw).expect("non-empty query");

        let m = Meeting::new("Daily", "manual");
        meetings.create(&m).expect("create meeting");

        let mut dictated = MeetingSegment::new(&m.id, "mic", 0, 1500, "segredo pessoal");
        dictated.excluded = true;
        segments.create(&dictated).expect("excluded segment");
        segments
            .create(&MeetingSegment::new(
                &m.id,
                "system",
                1500,
                3000,
                "pauta pública",
            ))
            .expect("public segment");

        assert!(meetings.search(&q("segredo")).expect("search").is_empty());
        assert_eq!(meetings.search(&q("pauta")).expect("search").len(), 1);

        // The UPDATE trigger must also respect the flag: un-excluding the
        // row indexes it, re-excluding removes it again.
        conn.execute(
            "UPDATE meeting_segments SET excluded = 0 WHERE id = ?1",
            params![dictated.id],
        )
        .expect("un-exclude");
        assert_eq!(meetings.search(&q("segredo")).expect("search").len(), 1);
        conn.execute(
            "UPDATE meeting_segments SET excluded = 1 WHERE id = ?1",
            params![dictated.id],
        )
        .expect("re-exclude");
        assert!(meetings.search(&q("segredo")).expect("search").is_empty());
    }

    #[test]
    fn app_exe_path_round_trips_through_create_and_update() {
        let conn = setup();
        let meetings = SqliteMeetingRepository::new(&conn);
        let mut m = Meeting::new("Daily", "auto_prompt");
        assert_eq!(m.app_exe_path, None, "manual meetings have no path");
        m.app_exe = Some("Zoom.exe".to_string());
        m.app_exe_path = Some(r"C:\apps\Zoom.exe".to_string());
        meetings.create(&m).expect("create");

        let fetched = meetings.get(&m.id).expect("get").expect("exists");
        assert_eq!(fetched.app_exe_path.as_deref(), Some(r"C:\apps\Zoom.exe"));

        let mut edited = fetched;
        edited.app_exe_path = None;
        meetings.update(&edited).expect("update");
        assert_eq!(
            meetings
                .get(&m.id)
                .expect("get")
                .expect("exists")
                .app_exe_path,
            None
        );
    }

    #[test]
    fn clear_audio_dir_nulls_only_the_audio_reference() {
        // T-115: after the audio-only delete the row keeps its transcript
        // data — only `audio_dir` is cleared.
        let conn = setup();
        let meetings = SqliteMeetingRepository::new(&conn);
        let mut m = Meeting::new("Daily", "manual");
        m.audio_dir = Some("audio/meetings/x".to_string());
        m.summary_md = Some("resumo".to_string());
        meetings.create(&m).expect("create");

        meetings.clear_audio_dir(&m.id).expect("clear_audio_dir");

        let fetched = meetings.get(&m.id).expect("get").expect("exists");
        assert_eq!(fetched.audio_dir, None);
        assert_eq!(fetched.summary_md.as_deref(), Some("resumo"));
        // Unknown id is a no-op, matching the other setters.
        meetings.clear_audio_dir("missing").expect("no-op");
    }

    #[test]
    fn invalid_status_is_rejected_by_check() {
        let conn = setup();
        let meetings = SqliteMeetingRepository::new(&conn);
        let mut m = Meeting::new("X", "manual");
        m.status = "bogus".to_string();
        assert!(meetings.create(&m).is_err());
    }

    #[test]
    fn app_rules_find_by_exe_is_case_insensitive() {
        let conn = setup();
        let rules = SqliteMeetingAppRuleRepository::new(&conn);
        // Migration 10 ships builtin rules — count relative to them and use
        // exes outside the seeded set.
        let seeded = rules.list().expect("list").len();

        rules
            .create(&MeetingAppRule::new("MyApp.exe", "My App", "ask"))
            .expect("create");
        let mut meet = MeetingAppRule::new("other.exe", "Other Meet", "ask");
        meet.title_pattern = Some("Meet".to_string());
        meet.builtin = true;
        rules.create(&meet).expect("create");

        assert_eq!(rules.find_by_exe("myapp.EXE").expect("find").len(), 1);
        assert_eq!(rules.list().expect("list").len(), seeded + 2);

        let created_id = rules.find_by_exe("MyApp.exe").expect("find")[0].id.clone();
        rules.delete(&created_id).expect("delete");
        assert_eq!(rules.list().expect("list").len(), seeded + 1);
    }

    #[test]
    fn app_rule_set_action_updates_only_the_action() {
        // T-061: `detector_respond` "always"/"never" and the rules screen land
        // here — builtin rows are editable in action, just not deletable.
        let conn = setup();
        let rules = SqliteMeetingAppRuleRepository::new(&conn);
        let zoom = rules
            .find_by_exe("Zoom.exe")
            .expect("find")
            .into_iter()
            .next()
            .expect("builtin Zoom rule exists");

        rules
            .set_action(&zoom.id, "auto_start")
            .expect("set_action");
        let updated = rules.find_by_exe("zoom.exe").expect("find")[0].clone();
        assert_eq!(updated.action, "auto_start");
        assert_eq!(updated.label, zoom.label);
        assert_eq!(updated.title_pattern, zoom.title_pattern);
        assert!(updated.builtin);

        // Unknown id is a no-op, not an error (the command layer checks
        // existence separately for a proper NotFound).
        rules.set_action("missing", "ignore").expect("no-op");
    }

    #[test]
    fn builtin_meeting_app_rules_are_seeded_by_migration() {
        // T-060 / spec F008: the closed v1 set — Zoom, Teams (packaged key +
        // both exes), Meet on the five browsers, Webex.
        let conn = setup();
        let rules = SqliteMeetingAppRuleRepository::new(&conn);
        let all = rules.list().expect("list seeded rules");

        assert_eq!(all.len(), 11);
        for r in &all {
            assert!(r.builtin, "{} must be builtin", r.id);
            assert_eq!(r.action, "ask");
            assert!(r.id.starts_with("builtin-"));
        }

        // Exe-only rules carry no title pattern; browser rules require one
        // (the classifier enforces that browsers can never match on exe alone).
        let by_exe = |exe: &str| {
            rules
                .find_by_exe(exe)
                .expect("find_by_exe")
                .into_iter()
                .next()
        };
        assert!(by_exe("Zoom.exe").unwrap().title_pattern.is_none());
        assert_eq!(
            by_exe("MSTeams").unwrap().label,
            "Microsoft Teams",
            "packaged Teams ConsentStore key"
        );
        let chrome = by_exe("chrome.exe").unwrap();
        assert_eq!(chrome.label, "Google Meet");
        let pattern = chrome.title_pattern.expect("browser rule needs a pattern");
        let re = regex::Regex::new(&pattern).expect("seeded pattern must compile");
        assert!(re.is_match("Meet - abc-defg-hij"));
        assert!(re.is_match("meet.google.com call"));
        assert!(!re.is_match("YouTube - Google Chrome"));
        assert!(by_exe("CiscoCollabHost.exe").is_some());
        assert!(by_exe("webexmta.exe").is_some());
    }

    #[test]
    fn mark_speech_excluded_flags_only_overlapping_mic_speech() {
        // FR-009-10 / AC-009-03 (T-065): when a dictation interval closes,
        // mic speech overlapping it is excluded — system rows, markers and
        // non-overlapping segments are untouched.
        let conn = setup();
        let meetings = SqliteMeetingRepository::new(&conn);
        let segments = SqliteMeetingSegmentRepository::new(&conn);
        let m = Meeting::new("Standup", "manual");
        meetings.create(&m).expect("create meeting");

        let inside = MeetingSegment::new(&m.id, "mic", 1000, 2000, "ditado");
        let touching = MeetingSegment::new(&m.id, "mic", 900, 1500, "borda");
        let before = MeetingSegment::new(&m.id, "mic", 0, 900, "antes");
        let sys = MeetingSegment::new(&m.id, "system", 1000, 2000, "outros");
        let mut marker = MeetingSegment::new(&m.id, "mic", 1000, 2000, "Ditado");
        marker.kind = "dictation_marker".to_string();
        for seg in [&inside, &touching, &before, &sys, &marker] {
            segments.create(seg).expect("insert");
        }

        let marked = segments
            .mark_speech_excluded(&m.id, "mic", 1000, 2000)
            .expect("mark excluded");
        assert_eq!(marked, 2, "inside + touching overlap the interval");

        let segs = segments.list_by_meeting(&m.id).expect("list");
        let by_text = |text: &str| segs.iter().find(|s| s.text == text).expect("row");
        assert!(by_text("ditado").excluded);
        assert!(by_text("borda").excluded);
        assert!(!by_text("antes").excluded, "end_ms == start → no overlap");
        assert!(!by_text("outros").excluded, "system never excluded");
        assert!(!by_text("Ditado").excluded, "markers are not speech");
    }
}
