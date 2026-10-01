//! Assistant context (FR-012-19): at send time the provider request carries
//! a bounded, local snapshot of the user's recent dictations and meetings —
//! titles, summaries, "Minhas notas" and short transcript excerpts — so the
//! assistant can answer "what did I dictate earlier?" or "what was decided
//! in yesterday's meeting?".
//!
//! Fail-open by design: any DB/lookup error yields an empty block — context
//! must never block or fail a turn. Content is never logged (T-003 policy);
//! it only leaves the machine inside the explicit provider call the user
//! triggered (FR-012-18).

use std::sync::Arc;
use tauri::{AppHandle, Manager};

use crate::db::meetings::{
    MeetingRepository, MeetingSegmentRepository, SqliteMeetingRepository,
    SqliteMeetingSegmentRepository,
};
use crate::db::notes::{NoteRepository, SqliteNoteRepository};
use crate::managers::history::HistoryManager;

/// Hard cap on the whole context block — `turn::truncate_history` reserves
/// this inside `MAX_INPUT_CHARS` so context + history + system prompt can
/// never blow the request budget.
pub(crate) const CONTEXT_MAX_CHARS: usize = 24_000;

/// Per-item caps — a single dictation/meeting can never starve the rest.
const DICTATION_MAX_CHARS: usize = 500;
const DICTATION_COUNT: usize = 15;
const MEETING_COUNT: usize = 6;
const SUMMARY_MAX_CHARS: usize = 1_200;
const NOTES_MAX_CHARS: usize = 800;
/// Transcript excerpts only for the newest few meetings — they are the
/// likely "what was said?" targets and full transcripts would blow the
/// budget.
const TRANSCRIPT_MEETING_COUNT: usize = 2;
const TRANSCRIPT_MAX_CHARS: usize = 1_500;

/// Char-safe truncation with an ellipsis marker.
fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

/// Keep the *tail* of a text within `max` chars (transcript excerpts are
/// most useful at the end — the conclusion of the meeting).
fn truncate_tail(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let mut out = String::from("…");
    out.extend(text.chars().skip(count - max));
    out
}

fn fmt_timestamp(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

/// Append `section` to `out` while the running total stays under
/// `CONTEXT_MAX_CHARS`.
fn push_bounded(out: &mut String, section: &str) -> bool {
    if out.chars().count() + section.chars().count() > CONTEXT_MAX_CHARS {
        return false;
    }
    out.push_str(section);
    true
}

/// Recent dictations, newest first — `final_text` (post-processed when it
/// exists) so the assistant sees what the user actually delivered.
async fn dictations_block(app: &AppHandle) -> String {
    let hm = app.state::<Arc<HistoryManager>>();
    let entries = hm
        .get_history_entries(None, Some(DICTATION_COUNT))
        .await
        .map(|page| page.entries)
        .unwrap_or_default();
    let mut out = String::new();
    for entry in entries {
        let text = entry.final_text.trim();
        if text.is_empty() {
            continue;
        }
        let title = entry.title.trim();
        let label = if title.is_empty() {
            "dictation".to_string()
        } else {
            title.to_string()
        };
        let _ = std::fmt::Write::write_fmt(
            &mut out,
            format_args!(
                "- [{}] {}: \"{}\"\n",
                fmt_timestamp(entry.timestamp),
                label,
                truncate(text, DICTATION_MAX_CHARS)
            ),
        );
    }
    out
}

/// Transcript excerpt for one meeting — the tail of the speech segments
/// (final, non-excluded, non-marker rows only).
fn transcript_excerpt(conn: &rusqlite::Connection, meeting_id: &str) -> String {
    let segments = SqliteMeetingSegmentRepository::new(conn)
        .list_by_meeting(meeting_id)
        .unwrap_or_default();
    let mut text = String::new();
    for seg in segments
        .iter()
        .filter(|s| s.kind == "speech" && s.is_final && !s.excluded)
    {
        if let Some(speaker) = seg.speaker.as_deref().filter(|s| !s.is_empty()) {
            text.push_str(speaker);
            text.push_str(": ");
        }
        text.push_str(seg.text.trim());
        text.push(' ');
    }
    truncate_tail(text.trim(), TRANSCRIPT_MAX_CHARS)
}

/// Recent meetings — title/date/status, plus summary, "Minhas notas" and a
/// transcript excerpt when they exist.
fn meetings_block(app: &AppHandle) -> String {
    let conn = match crate::meeting::session::open_session_db(app) {
        Ok(conn) => conn,
        Err(_) => return String::new(),
    };
    let meetings = SqliteMeetingRepository::new(&conn)
        .list()
        .unwrap_or_default();
    let mut out = String::new();
    let mut transcripts_left = TRANSCRIPT_MEETING_COUNT;
    for meeting in meetings.into_iter().take(MEETING_COUNT) {
        let mut item = format!(
            "### \"{}\" ({}, {})\n",
            meeting.title,
            fmt_timestamp(meeting.started_at),
            meeting.status
        );
        if let Some(summary) = meeting
            .summary_md
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            item.push_str(&format!(
                "Summary: {}\n",
                truncate(summary, SUMMARY_MAX_CHARS)
            ));
        }
        let notes: String = SqliteNoteRepository::new(&conn)
            .list_by_meeting(&meeting.id)
            .unwrap_or_default()
            .into_iter()
            .map(|n| n.body_md.trim().to_string())
            .filter(|b| !b.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        if !notes.is_empty() {
            item.push_str(&format!("Notes: {}\n", truncate(&notes, NOTES_MAX_CHARS)));
        }
        if transcripts_left > 0 && meeting.status == "ready" {
            transcripts_left -= 1;
            let excerpt = transcript_excerpt(&conn, &meeting.id);
            if !excerpt.is_empty() {
                item.push_str(&format!("Transcript excerpt: {excerpt}\n"));
            }
        }
        out.push_str(&item);
    }
    out
}

/// The context block appended to the assistant system prompt, or `""` when
/// there is nothing worth sending (fresh install, empty history).
pub(crate) async fn build_context_block(app: &AppHandle) -> String {
    let dictations = dictations_block(app).await;
    // SQLite reads are sync and tiny — run them off the async worker anyway
    // so a slow disk can never stall the runtime.
    let app_for_meetings = app.clone();
    let meetings = tauri::async_runtime::spawn_blocking(move || meetings_block(&app_for_meetings))
        .await
        .unwrap_or_default();

    if dictations.is_empty() && meetings.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "## Local context — the user's recent dictations and meetings\n\
         Answer questions about them from this data; if it is not here, say so \
         instead of inventing.\n\n",
    );
    if !dictations.is_empty() {
        push_bounded(&mut out, "### Recent dictations\n");
        push_bounded(&mut out, &dictations);
        push_bounded(&mut out, "\n");
    }
    if !meetings.is_empty() {
        push_bounded(&mut out, "### Recent meetings\n");
        push_bounded(&mut out, &meetings);
    }
    out
}
