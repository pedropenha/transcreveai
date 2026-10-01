//! Pure prompt/formatting logic for FR-009-17/18/20 — no I/O, fully unit
//! tested. The worker in [`super`] feeds meeting rows/segments in and ships
//! the produced `LlmRequest`s to `llm::router`.
//!
//! Layout of a summary call:
//!
//! - `system` — the selected `summary_templates.prompt` (FR-009-17); the
//!   seeded pt-BR default already carries the output skeleton + rules.
//! - `user` — "Minhas notas" (contexto prioritário, FR-009-17) then the
//!   timestamped transcript (`[mm:ss] Falante: texto`).
//!
//! Map-reduce (FR-009-18): when the formatted transcript exceeds
//! [`CONTEXT_BUDGET_CHARS`] the transcript is split into ~20-minute windows
//! ([`MAP_CHUNK_MS`]), each chunk produces a partial summary, and a final
//! consolidation call merges them under the same template.

use crate::db::meetings::MeetingSegment;
use crate::meeting::blocks::Track;

/// FR-009-18: map-reduce chunks span ~20 minutes of meeting time.
pub const MAP_CHUNK_MS: i64 = 20 * 60_000;

/// Conservative context budget in *characters* (~30k tokens at chars/4 —
/// there is no tokenizer helper in `llm/`, so the estimate stays coarse and
/// low on purpose: a longer transcript triggers map-reduce early).
pub const CONTEXT_BUDGET_CHARS: usize = 120_000;

/// Safety bound on reduce rounds for pathological meetings (each round
/// shrinks the input by roughly `CONTEXT_BUDGET_CHARS / partial size`).
pub const MAX_REDUCE_ROUNDS: usize = 4;

/// The transcript carries only `speech` rows — gap/dictation markers are
/// meeting-clock bookkeeping, not spoken content for the summary.
fn transcript_lines(segments: &[MeetingSegment]) -> Vec<String> {
    segments
        .iter()
        .filter(|s| s.kind == "speech" && !s.text.trim().is_empty())
        .map(|s| {
            format!(
                "[{}] {}: {}",
                timestamp_mmss(s.start_ms),
                speaker_label(s),
                s.text.trim()
            )
        })
        .collect()
}

/// `[mm:ss]` with `mm` unbounded (meetings past an hour keep counting).
pub fn timestamp_mmss(ms: i64) -> String {
    let total_secs = ms.max(0) / 1000;
    format!("{:02}:{:02}", total_secs / 60, total_secs % 60)
}

/// Display speaker: persisted label if present (diarization lands in P1),
/// else the track default — `mic` is "Você", `system` is "Outros".
fn speaker_label(segment: &MeetingSegment) -> &str {
    segment
        .speaker
        .as_deref()
        .unwrap_or(match segment.track.as_str() {
            "mic" => "Você",
            _ => "Outros",
        })
}

/// The speaker label stamped on segments this pipeline writes (T-064 wrote
/// none; diarization would refine `system` rows to "Falante N" in P1).
pub fn speaker_for_track(track: Track) -> &'static str {
    match track {
        Track::Mic => "Você",
        Track::System => "Outros",
    }
}

/// The full timestamped transcript for the summary prompt.
pub fn format_transcript(segments: &[MeetingSegment]) -> String {
    transcript_lines(segments).join("\n")
}

/// Split speech segments into ~`chunk_ms` windows — FR-009-18's ~20 min.
/// Boundaries align to segment starts; `max_chars` additionally splits a
/// window that would exceed the context budget on its own (dense speech).
pub fn split_for_map(
    segments: &[MeetingSegment],
    chunk_ms: i64,
    max_chars: usize,
) -> Vec<Vec<MeetingSegment>> {
    let speech: Vec<&MeetingSegment> = segments
        .iter()
        .filter(|s| s.kind == "speech" && !s.text.trim().is_empty())
        .collect();
    let mut chunks: Vec<Vec<MeetingSegment>> = Vec::new();
    let mut current: Vec<MeetingSegment> = Vec::new();
    let mut window_start: Option<i64> = None;
    let mut current_chars = 0usize;
    for segment in speech {
        let over_time = window_start
            .map(|start| segment.start_ms - start >= chunk_ms)
            .unwrap_or(false);
        let over_chars =
            !current.is_empty() && current_chars + segment.text.chars().count() > max_chars;
        if over_time || over_chars {
            chunks.push(std::mem::take(&mut current));
            window_start = None;
            current_chars = 0;
        }
        if window_start.is_none() {
            window_start = Some(segment.start_ms);
        }
        current_chars += segment.text.chars().count();
        current.push(segment.clone());
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

/// Group partial summaries so each consolidation call stays under the
/// context budget — the reduce half of the map-reduce.
pub fn group_strings_by_chars(items: &[String], max_chars: usize) -> Vec<Vec<String>> {
    let mut groups: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut current_chars = 0usize;
    for item in items {
        let len = item.chars().count();
        if !current.is_empty() && current_chars + len > max_chars {
            groups.push(std::mem::take(&mut current));
            current_chars = 0;
        }
        current_chars += len;
        current.push(item.clone());
    }
    if !current.is_empty() {
        groups.push(current);
    }
    groups
}

/// `system` for every summary-shaped call — the selected template verbatim
/// (it already encodes the output skeleton and the FR-009-17 rules).
pub fn summary_system_prompt(template_prompt: &str) -> String {
    template_prompt.trim().to_string()
}

/// `user` for a single-shot summary: notes first (FR-009-17 makes "Minhas
/// notas" the priority context), then the timestamped transcript.
pub fn summary_user_prompt(notes: &str, transcript: &str) -> String {
    let mut body = String::new();
    let notes = notes.trim();
    if !notes.is_empty() {
        body.push_str("Minhas notas (contexto prioritário):\n");
        body.push_str(notes);
        body.push_str("\n\n");
    }
    body.push_str("Transcrição:\n");
    body.push_str(transcript);
    body
}

/// `system` for a map step over a 20-minute window: extract, don't format —
/// the consolidation call owns the template.
pub fn map_system_prompt() -> String {
    "Você é o assistente de atas do Transcreve.ai. A transcrição a seguir é um \
     trecho de uma reunião mais longa. Extraia os pontos importantes deste \
     trecho em tópicos concisos: decisões, tarefas e responsáveis, pontos em \
     aberto e os temas discutidos com seus horários (mm:ss). Escreva no \
     idioma predominante do trecho. Não invente responsáveis nem prazos."
        .to_string()
}

/// `system` for the consolidation: partial summaries + notes in, the
/// selected template formats the output.
pub fn reduce_user_prompt(notes: &str, partials: &[String]) -> String {
    let mut body = String::new();
    let notes = notes.trim();
    if !notes.is_empty() {
        body.push_str("Minhas notas (contexto prioritário):\n");
        body.push_str(notes);
        body.push_str("\n\n");
    }
    body.push_str(
        "Resumos parciais da reunião (um por trecho, em ordem cronológica) — \
         consolide-os no formato pedido, sem perder decisões, tarefas nem \
         horários:\n\n",
    );
    for (i, partial) in partials.iter().enumerate() {
        body.push_str(&format!("--- Trecho {} ---\n{}\n\n", i + 1, partial.trim()));
    }
    body
}

/// Suggested-title call (FR-009-16 step 5): short, plain text, no Markdown.
pub fn title_system_prompt() -> String {
    "Sugira um título curto (até 6 palavras) para esta reunião com base no \
     resumo. Responda apenas o título, sem aspas, sem Markdown e sem ponto \
     final."
        .to_string()
}

/// The suggested title is usable only while the row still shows the default
/// placeholder `"<App> · <dd/mm hh:mm>"` / `"Reunião · <dd/mm hh:mm>"`
/// (`session::default_title`, FR-009-12) — a user edit must never be
/// clobbered (FR-009-20 regenerations included).
pub fn title_is_default(title: &str) -> bool {
    let Some((_, stamp)) = title.rsplit_once(" · ") else {
        return false;
    };
    // `dd/mm hh:mm` — exactly 11 chars: 2 digits, '/', 2 digits, ' ',
    // 2 digits, ':', 2 digits.
    let chars: Vec<char> = stamp.chars().collect();
    if chars.len() != 11 {
        return false;
    }
    let digit_at = |i: usize| chars.get(i).is_some_and(|c| c.is_ascii_digit());
    digit_at(0)
        && digit_at(1)
        && chars.get(2) == Some(&'/')
        && digit_at(3)
        && digit_at(4)
        && chars.get(5) == Some(&' ')
        && digit_at(6)
        && digit_at(7)
        && chars.get(8) == Some(&':')
        && digit_at(9)
        && digit_at(10)
}

/// Clean up an LLM title answer: first line only, no quotes/Markdown,
/// bounded length. `None` when nothing usable came back.
pub fn sanitize_suggested_title(raw: &str) -> Option<String> {
    let line = raw.lines().next()?.trim();
    let cleaned = line
        .trim_matches(|c: char| c == '"' || c == '\'' || c == '#' || c == '*' || c == '.')
        .trim();
    if cleaned.is_empty() {
        return None;
    }
    const MAX_TITLE_CHARS: usize = 80;
    let truncated: String = cleaned.chars().take(MAX_TITLE_CHARS).collect();
    Some(truncated)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(track: &str, start_ms: i64, text: &str) -> MeetingSegment {
        MeetingSegment::new("m1", track, start_ms, start_ms + 1000, text)
    }

    #[test]
    fn transcript_is_timestamped_per_speaker() {
        let segments = vec![
            seg("mic", 5_000, "bom dia"),
            seg("system", 61_500, "oi pessoal"),
        ];
        let out = format_transcript(&segments);
        assert_eq!(out, "[00:05] Você: bom dia\n[01:01] Outros: oi pessoal");
    }

    #[test]
    fn transcript_skips_markers_and_blank_text() {
        let mut gap = seg("mic", 10_000, "");
        gap.kind = "gap_marker".to_string();
        let blank = seg("system", 20_000, "   ");
        let out = format_transcript(&[gap, blank, seg("mic", 30_000, "ok")]);
        assert_eq!(out, "[00:30] Você: ok");
    }

    #[test]
    fn timestamps_run_past_an_hour() {
        assert_eq!(timestamp_mmss(0), "00:00");
        assert_eq!(timestamp_mmss(3_723_000), "62:03");
    }

    #[test]
    fn map_split_respects_the_20_minute_window() {
        // 45 min of segments: windows at 0–20, 20–40, 40+.
        let segments: Vec<_> = (0..45).map(|i| seg("mic", i * 60_000, "linha")).collect();
        let chunks = split_for_map(&segments, MAP_CHUNK_MS, usize::MAX);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].len(), 20);
        assert_eq!(chunks[2].len(), 5);
        assert_eq!(chunks[1][0].start_ms, 20 * 60_000);
    }

    #[test]
    fn map_split_also_bounds_characters() {
        let big = "x".repeat(500);
        let segments: Vec<_> = (0..10).map(|i| seg("mic", i * 1_000, &big)).collect();
        let chunks = split_for_map(&segments, MAP_CHUNK_MS, 1_200);
        assert_eq!(chunks.len(), 5);
        assert!(chunks.iter().all(|c| c.len() <= 2));
    }

    #[test]
    fn user_prompt_puts_notes_first() {
        let prompt = summary_user_prompt("cliente pediu desconto", "[00:01] Você: oi");
        let notes_at = prompt.find("cliente pediu desconto").unwrap();
        let transcript_at = prompt.find("Transcrição:").unwrap();
        assert!(
            notes_at < transcript_at,
            "notes must precede the transcript"
        );
    }

    #[test]
    fn user_prompt_without_notes_is_just_the_transcript() {
        let prompt = summary_user_prompt("  ", "linha");
        assert_eq!(prompt, "Transcrição:\nlinha");
    }

    #[test]
    fn reduce_groups_stay_under_budget() {
        let partials: Vec<String> = (0..10).map(|_| "p".repeat(400)).collect();
        let groups = group_strings_by_chars(&partials, 1_000);
        assert_eq!(groups.len(), 5);
        assert!(groups
            .iter()
            .all(|g| g.iter().map(String::len).sum::<usize>() <= 1_000));
    }

    #[test]
    fn reduce_prompt_lists_partials_in_order() {
        let prompt = reduce_user_prompt("", &["a".to_string(), "b".to_string()]);
        assert!(prompt.contains("--- Trecho 1 ---\na"));
        assert!(prompt.contains("--- Trecho 2 ---\nb"));
    }

    #[test]
    fn default_title_shape_is_detected() {
        assert!(title_is_default("Google Meet · 25/03 14:05"));
        assert!(title_is_default("Reunião · 01/01 09:30"));
        // User edits are never the placeholder.
        assert!(!title_is_default("Weekly de produto"));
        assert!(!title_is_default("Google Meet · planning"));
        assert!(!title_is_default("Meet · 25/03/2025 14:05"));
        assert!(!title_is_default(""));
        assert!(!title_is_default("sem separador"));
    }

    #[test]
    fn suggested_title_is_sanitized() {
        assert_eq!(
            sanitize_suggested_title("\"Alinhamento de preços\"\nextra"),
            Some("Alinhamento de preços".to_string())
        );
        assert_eq!(
            sanitize_suggested_title("## Weekly ##"),
            Some("Weekly".to_string())
        );
        assert_eq!(sanitize_suggested_title("   \n"), None);
        assert_eq!(sanitize_suggested_title(""), None);
        // Long answers are truncated to a bounded title.
        let long = "título ".repeat(30);
        assert!(sanitize_suggested_title(&long).unwrap().chars().count() <= 80);
    }
}
