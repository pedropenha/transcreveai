//! Read-side view of a meeting for the Notetaker list and details panel
//! (F009 FR-009-25, UI redesign stage 4): the persisted row plus two derived
//! facts the UI would otherwise have to re-implement — the source app
//! (`app_exe` + `app_label`) and the single status chip of a list row.
//!
//! Everything here is pure (no db, no IPC) so the mapping is unit-tested; the
//! commands layer only loads rows and calls into it.

use serde::Serialize;
use specta::Type;

use crate::db::meetings::Meeting;

/// The app a meeting was started from (detector or toast), when known.
/// Manual / in-person meetings have none.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Type)]
pub struct SourceApp {
    /// Executable file name, e.g. `Zoom.exe` (`None` when only a label is
    /// known).
    pub exe: Option<String>,
    /// Friendly name, e.g. "Google Meet"; falls back to the exe name without
    /// its `.exe` suffix.
    pub name: String,
}

/// The one chip a Notetaker list row shows (design: "Estados da linha").
/// Labels live in the front-end: `processing` = "Transcrevendo",
/// `no_summary` = "Sem resumo", `failed` = "Falhou", `ready` = "Resumo pronto".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum MeetingListStatus {
    /// Being recorded now — shown in the "Agora" block, not in the list.
    Recording,
    /// Recording paused — same "Agora" block.
    Paused,
    /// Transcribing / summarizing (also a summary regeneration in flight).
    Processing,
    /// Transcript and notes exist but there is no summary (FR-009-21: no
    /// BYOK key). Clicking leads to Settings → Summaries.
    NoSummary,
    /// Processing failed (`error`), the app crashed mid-recording
    /// (`recovered`) or the summary failed. Retry with
    /// `meeting_retry_processing` when `meeting.status` is `error` /
    /// `recovered`, with `meeting_regenerate_summary` when it is `ready`.
    Failed,
    /// Transcript and summary are ready.
    Ready,
}

/// Collapses `meetings.status` × `meetings.summary_status` into one list
/// status. Unknown values (impossible under the table CHECKs) read as
/// `Failed` so a corrupt row is surfaced instead of looking healthy.
pub fn derive_list_status(
    status: &str,
    summary_status: &str,
    has_summary: bool,
) -> MeetingListStatus {
    match status {
        "recording" => MeetingListStatus::Recording,
        "paused" => MeetingListStatus::Paused,
        "processing" => MeetingListStatus::Processing,
        "error" | "recovered" => MeetingListStatus::Failed,
        "ready" => match summary_status {
            "pending" => MeetingListStatus::Processing,
            "disabled" => MeetingListStatus::NoSummary,
            "error" => MeetingListStatus::Failed,
            "ready" if has_summary => MeetingListStatus::Ready,
            "ready" => MeetingListStatus::NoSummary,
            _ => MeetingListStatus::Failed,
        },
        _ => MeetingListStatus::Failed,
    }
}

fn non_blank(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|v| !v.is_empty())
}

fn strip_exe_suffix(exe: &str) -> &str {
    match exe.len().checked_sub(4).and_then(|at| exe.get(at..)) {
        Some(ext) if ext.eq_ignore_ascii_case(".exe") => &exe[..exe.len() - 4],
        _ => exe,
    }
}

/// `source_app` of a meeting: `None` for manual / in-person meetings.
pub fn source_app(meeting: &Meeting) -> Option<SourceApp> {
    let exe = non_blank(meeting.app_exe.as_deref());
    let label = non_blank(meeting.app_label.as_deref());
    let name = label.or_else(|| exe.map(strip_exe_suffix))?;
    Some(SourceApp {
        exe: exe.map(str::to_string),
        name: name.to_string(),
    })
}

/// Icon data URI for a meeting's source app via `resolve(label, exe, path)`
/// (the app-icon cache in production, a double in tests). `None` when the
/// meeting has no source exe, no stored path, or the resolver has no icon
/// (known apps / browsers use the front-end's embedded logos).
pub fn source_icon(
    meeting: &Meeting,
    resolve: impl FnOnce(&str, &str, Option<&str>) -> Option<String>,
) -> Option<String> {
    let exe = non_blank(meeting.app_exe.as_deref())?;
    let label = non_blank(meeting.app_label.as_deref()).unwrap_or("");
    resolve(label, exe, meeting.app_exe_path.as_deref())
}

/// A meeting row for the list/search commands: the full [`Meeting`] (flattened,
/// so existing consumers keep working) plus the derived fields.
#[derive(Clone, Debug, PartialEq, Serialize, Type)]
pub struct MeetingListItem {
    #[serde(flatten)]
    pub meeting: Meeting,
    pub source_app: Option<SourceApp>,
    pub list_status: MeetingListStatus,
}

impl From<Meeting> for MeetingListItem {
    fn from(meeting: Meeting) -> Self {
        let list_status = list_status_of(&meeting);
        let source_app = source_app(&meeting);
        Self {
            meeting,
            source_app,
            list_status,
        }
    }
}

/// [`derive_list_status`] applied to a row.
pub fn list_status_of(meeting: &Meeting) -> MeetingListStatus {
    let has_summary = meeting
        .summary_md
        .as_deref()
        .is_some_and(|s| !s.trim().is_empty());
    derive_list_status(&meeting.status, &meeting.summary_status, has_summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meeting(status: &str, summary_status: &str, summary: Option<&str>) -> Meeting {
        let mut m = Meeting::new("Daily", "auto_prompt");
        m.status = status.to_string();
        m.summary_status = summary_status.to_string();
        m.summary_md = summary.map(str::to_string);
        m
    }

    #[test]
    fn status_matrix_matches_the_design_chips() {
        use MeetingListStatus::*;
        let cases = [
            ("recording", "pending", None, Recording),
            ("paused", "pending", None, Paused),
            ("processing", "pending", None, Processing),
            ("error", "pending", None, Failed),
            ("recovered", "pending", None, Failed),
            ("ready", "ready", Some("## Resumo"), Ready),
            ("ready", "ready", Some("   "), NoSummary),
            ("ready", "ready", None, NoSummary),
            ("ready", "disabled", None, NoSummary),
            ("ready", "error", None, Failed),
            ("ready", "pending", None, Processing),
            ("bogus", "ready", Some("x"), Failed),
            ("ready", "bogus", Some("x"), Failed),
        ];
        for (status, summary_status, summary, expected) in cases {
            assert_eq!(
                list_status_of(&meeting(status, summary_status, summary)),
                expected,
                "{status}/{summary_status}/{summary:?}"
            );
        }
    }

    #[test]
    fn source_app_prefers_label_and_falls_back_to_exe_stem() {
        let mut m = Meeting::new("x", "auto_prompt");
        assert_eq!(source_app(&m), None, "manual meeting has no source");

        m.app_exe = Some("chrome.exe".to_string());
        m.app_label = Some("Google Meet".to_string());
        assert_eq!(
            source_app(&m),
            Some(SourceApp {
                exe: Some("chrome.exe".to_string()),
                name: "Google Meet".to_string()
            })
        );

        m.app_label = Some("   ".to_string());
        assert_eq!(source_app(&m).expect("source").name, "chrome");

        m.app_exe = None;
        m.app_label = Some("Zoom".to_string());
        let only_label = source_app(&m).expect("source");
        assert_eq!(only_label.exe, None);
        assert_eq!(only_label.name, "Zoom");
    }

    #[test]
    fn exe_suffix_is_stripped_case_insensitively_and_char_boundary_safe() {
        assert_eq!(strip_exe_suffix("Zoom.EXE"), "Zoom");
        assert_eq!(strip_exe_suffix("MSTeams"), "MSTeams");
        assert_eq!(strip_exe_suffix("ab"), "ab");
        assert_eq!(strip_exe_suffix("ação"), "ação");
    }

    #[test]
    fn list_item_serializes_flat_with_derived_fields() {
        let mut m = meeting("ready", "disabled", None);
        m.app_exe = Some("Zoom.exe".to_string());
        m.app_exe_path = Some(r"C:\apps\Zoom.exe".to_string());
        let json = serde_json::to_value(MeetingListItem::from(m)).expect("serialize");
        assert_eq!(
            json["title"], "Daily",
            "Meeting fields stay at the top level"
        );
        assert_eq!(json["list_status"], "no_summary");
        assert_eq!(json["source_app"]["exe"], "Zoom.exe");
        assert_eq!(json["source_app"]["name"], "Zoom");
        assert!(
            json.get("app_exe_path").is_none(),
            "the exe path never crosses IPC"
        );
    }

    #[test]
    fn source_icon_passes_label_exe_and_path_to_the_resolver() {
        let mut m = Meeting::new("x", "auto_prompt");
        m.app_exe = Some("Foo.exe".to_string());
        m.app_label = Some("Foo".to_string());
        m.app_exe_path = Some(r"C:\apps\Foo.exe".to_string());
        let icon = source_icon(&m, |label, exe, path| {
            assert_eq!(
                (label, exe, path),
                ("Foo", "Foo.exe", Some(r"C:\apps\Foo.exe"))
            );
            Some("data:image/png;base64,AAAA".to_string())
        });
        assert_eq!(icon.as_deref(), Some("data:image/png;base64,AAAA"));
    }

    #[test]
    fn source_icon_falls_back_to_none_without_source_or_icon() {
        let manual = Meeting::new("x", "manual");
        assert_eq!(
            source_icon(&manual, |_, _, _| panic!("no exe, no extraction")),
            None
        );

        let mut legacy = Meeting::new("x", "auto_prompt");
        legacy.app_exe = Some("Foo.exe".to_string());
        // Meeting from before migration 15: no stored path → resolver gets
        // `None` and yields no icon → the UI shows its monogram.
        assert_eq!(
            source_icon(&legacy, |_, _, path| {
                assert_eq!(path, None);
                None
            }),
            None
        );
    }
}
