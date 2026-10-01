//! "Copiar como Markdown" assembler (T-068; FR-009-23, AC-009-07).
//!
//! Pure functions — [`render_meeting_markdown`] takes the meeting row, the
//! "Minhas notas" body, the ordered segments, a pre-formatted start-time
//! label and a [`MarkdownLabels`] table (resolved from `app_language`) and
//! produces the exact Markdown document. No db access, no `AppHandle`, so
//! the whole layout is unit-testable against fixtures.
//!
//! Structure (contract with FR-009-23):
//!
//! ```markdown
//! # <title>
//!
//! - **Data:** <start> · **Duração:** <dur> · **App:** <app>
//!
//! ## Minhas notas
//! ## Resumo
//! ## Transcrição
//! ```
//!
//! Transcript lines are `[mm:ss] Falante: texto` (`[h:mm:ss]` past one
//! hour); `dictation_marker` and `gap_marker` segments render as italic
//! markers (`*Ditado*`, `*pausa*`) instead of fake speaker lines.

use crate::db::meetings::{Meeting, MeetingSegment};

/// Localized strings stamped into the exported document. `&'static str`
/// keeps the assembler allocation-free on the label side.
#[derive(Clone, Copy, Debug)]
pub struct MarkdownLabels {
    pub date: &'static str,
    pub duration: &'static str,
    pub app: &'static str,
    pub notes: &'static str,
    pub summary: &'static str,
    pub transcript: &'static str,
    /// Fallback speaker for `mic` segments without a `speaker` column.
    pub speaker_mic: &'static str,
    /// Fallback speaker for `system` segments without a `speaker` column.
    pub speaker_system: &'static str,
    /// `dictation_marker` segments (FR-009-10).
    pub dictation: &'static str,
    /// `gap_marker` segments (FR-009-04/06).
    pub pause: &'static str,
}

/// `lang` is the normalized `app_language` (`"pt-BR"` or `"en"` — see
/// `settings::normalize_app_language`); anything non-Portuguese falls back
/// to English, mirroring `meeting::session::toast_message`.
pub fn markdown_labels(lang: &str) -> MarkdownLabels {
    if lang == "pt-BR" {
        MarkdownLabels {
            date: "Data",
            duration: "Duração",
            app: "App",
            notes: "Minhas notas",
            summary: "Resumo",
            transcript: "Transcrição",
            speaker_mic: "Você",
            speaker_system: "Sistema",
            dictation: "Ditado",
            pause: "pausa",
        }
    } else {
        MarkdownLabels {
            date: "Date",
            duration: "Duration",
            app: "App",
            notes: "My notes",
            summary: "Summary",
            transcript: "Transcript",
            speaker_mic: "You",
            speaker_system: "System",
            dictation: "Dictation",
            pause: "pause",
        }
    }
}

/// `start_ms` relative to the meeting start → `mm:ss`, or `h:mm:ss` once
/// the meeting runs past an hour (max duration is 4 h, FR-009-08).
fn segment_timestamp(start_ms: i64) -> String {
    let total_secs = start_ms.max(0) / 1000;
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

/// `ended_at - started_at` in seconds → `"32 min"` / `"1 h 12 min"`;
/// `"—"` while the meeting has no `ended_at` yet.
fn meeting_duration(meeting: &Meeting) -> String {
    let Some(secs) = meeting
        .ended_at
        .map(|ended| ended.saturating_sub(meeting.started_at))
        .filter(|secs| *secs >= 0)
    else {
        return "—".to_string();
    };
    let total_min = secs / 60;
    let (hours, minutes) = (total_min / 60, total_min % 60);
    if hours > 0 {
        format!("{hours} h {minutes} min")
    } else {
        format!("{minutes} min")
    }
}

/// The app line prefers the friendly label (`app_label`), then the exe —
/// `"—"` when the meeting was started manually with no detected app.
fn meeting_app(meeting: &Meeting) -> &str {
    meeting
        .app_label
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| meeting.app_exe.as_deref().filter(|s| !s.trim().is_empty()))
        .unwrap_or("—")
}

/// Speaker for a `speech` segment: the stored `speaker` wins (diarization
/// writes "Falante N"), otherwise the track fallback (`mic` → "Você",
/// `system` → "Sistema").
fn segment_speaker<'a>(segment: &'a MeetingSegment, labels: &MarkdownLabels) -> &'a str {
    segment
        .speaker
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(if segment.track == "mic" {
            labels.speaker_mic
        } else {
            labels.speaker_system
        })
}

/// Body text or the em-dash placeholder — an empty section still shows its
/// heading so the exported document always has the same skeleton.
fn body_or_placeholder(text: Option<&str>) -> &str {
    text.filter(|s| !s.trim().is_empty()).unwrap_or("—")
}

/// `# ` heading text can't span lines — collapse any newline the title may
/// carry so the document structure survives odd titles.
fn sanitize_heading(title: &str) -> String {
    title.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn render_meeting_markdown(
    meeting: &Meeting,
    notes_md: Option<&str>,
    segments: &[MeetingSegment],
    labels: &MarkdownLabels,
    started_at_label: &str,
) -> String {
    let mut out = String::new();

    out.push_str(&format!("# {}\n\n", sanitize_heading(&meeting.title)));
    out.push_str(&format!("- **{}:** {}\n", labels.date, started_at_label));
    out.push_str(&format!(
        "- **{}:** {}\n",
        labels.duration,
        meeting_duration(meeting)
    ));
    out.push_str(&format!("- **{}:** {}\n", labels.app, meeting_app(meeting)));

    out.push_str(&format!(
        "\n## {}\n\n{}\n",
        labels.notes,
        body_or_placeholder(notes_md)
    ));
    out.push_str(&format!(
        "\n## {}\n\n{}\n",
        labels.summary,
        body_or_placeholder(meeting.summary_md.as_deref())
    ));
    out.push_str(&format!("\n## {}\n\n", labels.transcript));

    if segments.is_empty() {
        out.push_str("—\n");
    } else {
        for segment in segments {
            // FR-009-10/AC-009-03: `excluded` mic rows are dictated notes —
            // the exported transcript shows the `dictation_marker` instead.
            if segment.excluded {
                continue;
            }
            let ts = segment_timestamp(segment.start_ms);
            match segment.kind.as_str() {
                "dictation_marker" => {
                    out.push_str(&format!("[{ts}] *{}*\n", labels.dictation));
                }
                "gap_marker" => {
                    out.push_str(&format!("[{ts}] *{}*\n", labels.pause));
                }
                _ => {
                    out.push_str(&format!(
                        "[{ts}] {}: {}\n",
                        segment_speaker(segment, labels),
                        segment.text
                    ));
                }
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_meeting() -> Meeting {
        let mut m = Meeting::new("Daily · 12/03 14:30", "manual");
        m.app_label = Some("Google Meet".to_string());
        m.started_at = 1_741_785_000; // fixed; the formatted label is injected
        m.ended_at = Some(1_741_785_000 + 32 * 60 + 15);
        m.status = "ready".to_string();
        m.summary_md = Some("- ship it Friday".to_string());
        m
    }

    fn fixture_segments(meeting_id: &str) -> Vec<MeetingSegment> {
        let mut s1 = MeetingSegment::new(meeting_id, "mic", 12_000, 15_000, "bom dia pessoal");
        s1.speaker = None;
        let mut s2 = MeetingSegment::new(meeting_id, "system", 15_500, 18_000, "oi, tudo bem?");
        s2.speaker = Some("Falante 1".to_string());
        let mut s3 = MeetingSegment::new(meeting_id, "mic", 60_000, 60_500, "");
        s3.kind = "dictation_marker".to_string();
        let mut s4 = MeetingSegment::new(meeting_id, "system", 150_000, 150_200, "");
        s4.kind = "gap_marker".to_string();
        vec![s1, s2, s3, s4]
    }

    #[test]
    fn renders_full_document_in_pt_br() {
        let meeting = fixture_meeting();
        let segments = fixture_segments(&meeting.id);
        let md = render_meeting_markdown(
            &meeting,
            Some("- cliente pediu desconto"),
            &segments,
            &markdown_labels("pt-BR"),
            "2025-03-12 14:30",
        );

        let expected = "# Daily · 12/03 14:30\n\n\
             - **Data:** 2025-03-12 14:30\n\
             - **Duração:** 32 min\n\
             - **App:** Google Meet\n\n\
             ## Minhas notas\n\n- cliente pediu desconto\n\n\
             ## Resumo\n\n- ship it Friday\n\n\
             ## Transcrição\n\n\
             [00:12] Você: bom dia pessoal\n\
             [00:15] Falante 1: oi, tudo bem?\n\
             [01:00] *Ditado*\n\
             [02:30] *pausa*\n";
        assert_eq!(md, expected);
    }

    #[test]
    fn renders_full_document_in_english() {
        let meeting = fixture_meeting();
        let segments = fixture_segments(&meeting.id);
        let md = render_meeting_markdown(
            &meeting,
            Some("- client asked for a discount"),
            &segments,
            &markdown_labels("en"),
            "2025-03-12 14:30",
        );

        assert!(md.starts_with("# Daily · 12/03 14:30\n\n- **Date:** 2025-03-12 14:30\n"));
        assert!(md.contains("- **Duration:** 32 min\n"));
        assert!(md.contains("## My notes\n\n- client asked for a discount\n"));
        assert!(md.contains("## Transcript\n\n"));
        assert!(md.contains("[00:12] You: bom dia pessoal\n"));
        assert!(md.contains("[00:15] Falante 1: oi, tudo bem?\n"));
        assert!(md.contains("[01:00] *Dictation*\n"));
        assert!(md.contains("[02:30] *pause*\n"));
        // Any unrecognized language folds to English.
        assert_eq!(markdown_labels("de").notes, "My notes");
    }

    #[test]
    fn missing_sections_render_placeholder() {
        let mut meeting = fixture_meeting();
        meeting.summary_md = None;
        meeting.app_label = None;
        meeting.app_exe = None;
        meeting.ended_at = None;
        let md = render_meeting_markdown(
            &meeting,
            None,
            &[],
            &markdown_labels("pt-BR"),
            "2025-03-12 14:30",
        );

        assert!(md.contains("- **Duração:** —\n"));
        assert!(md.contains("- **App:** —\n"));
        assert!(md.contains("## Minhas notas\n\n—\n"));
        assert!(md.contains("## Resumo\n\n—\n"));
        assert!(md.contains("## Transcrição\n\n—\n"));
    }

    #[test]
    fn long_meetings_and_system_fallback_speaker() {
        let mut meeting = fixture_meeting();
        meeting.ended_at = Some(meeting.started_at + 4_320); // 1 h 12 min
        let mut seg = MeetingSegment::new(&meeting.id, "system", 3_661_000, 3_662_000, "fim");
        seg.speaker = None;
        let md = render_meeting_markdown(&meeting, None, &[seg], &markdown_labels("pt-BR"), "x");
        assert!(md.contains("- **Duração:** 1 h 12 min\n"));
        // 1:01:01 elapsed → h:mm:ss, system track without speaker → "Sistema".
        assert!(md.contains("[1:01:01] Sistema: fim\n"));
    }

    #[test]
    fn excluded_dictated_speech_stays_out_of_the_export() {
        // FR-009-10/AC-009-03: the "Ditado" marker renders, the dictated
        // text behind it does not.
        let meeting = fixture_meeting();
        let mut dictated = MeetingSegment::new(&meeting.id, "mic", 30_000, 31_000, "sigilo");
        dictated.excluded = true;
        let mut marker = MeetingSegment::new(&meeting.id, "mic", 30_000, 31_000, "Ditado");
        marker.kind = "dictation_marker".to_string();
        let md = render_meeting_markdown(
            &meeting,
            None,
            &[dictated, marker],
            &markdown_labels("pt-BR"),
            "x",
        );
        assert!(md.contains("*Ditado*"));
        assert!(!md.contains("sigilo"));
    }

    #[test]
    fn multiline_title_is_collapsed() {
        let mut meeting = fixture_meeting();
        meeting.title = "linha um\nlinha dois".to_string();
        let md = render_meeting_markdown(&meeting, None, &[], &markdown_labels("en"), "x");
        assert!(md.starts_with("# linha um linha dois\n"));
    }
}
