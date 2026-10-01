//! FR-009-16 steps 4–5 — summary (FR-009-17/18/19) and suggested title.
//!
//! All LLM traffic goes through `llm::router::complete_for_purpose`
//! (`LlmPurpose::Summary` / `Title`), which owns routing, BYOK lookup and the
//! transient-retry policy — this module only shapes requests and interprets
//! outcomes:
//!
//! - `MissingApiKey`/`Offline`/`Unsupported` and the pre-flight
//!   [`summary_status`] gate → `summary_status='disabled'`, meeting stays
//!   `ready`, `meeting_summary_disabled` toast (FR-009-21).
//! - Other failures → `summary_status='error'`; the caller decides whether
//!   that escalates to `status='error'` (full pass: yes — FR-009-22; summary
//!   regeneration: no, the meeting stays `ready`).
//!
//! "Minhas notas" (`notes` where `source='meeting'`) is read-only context —
//! this module never writes back to it (FR-009-19 / AC-009-06).

use std::time::Duration;

use crate::commands::llm::summary_status;
use crate::commands::{CommandError, CommandErrorCode};
use crate::db::meetings::{
    Meeting, MeetingRepository, MeetingSegment, MeetingSegmentRepository, SqliteMeetingRepository,
    SqliteMeetingSegmentRepository,
};
use crate::db::notes::{NoteRepository, SqliteNoteRepository};
use crate::db::summary_templates::{
    SqliteSummaryTemplateRepository, SummaryTemplateRepository, BUILTIN_DEFAULT_TEMPLATE_ID,
};
use crate::llm::types::{LlmError, LlmMessage, LlmPurpose, LlmRequest};
use crate::settings::get_settings;

use super::prompt::{
    format_transcript, group_strings_by_chars, map_system_prompt, reduce_user_prompt,
    sanitize_suggested_title, split_for_map, summary_system_prompt, summary_user_prompt,
    title_is_default, title_system_prompt, CONTEXT_BUDGET_CHARS, MAP_CHUNK_MS, MAX_REDUCE_ROUNDS,
};
use super::{SummaryOutcome, Worker};

/// Last-resort prompt when `summary_templates` is somehow empty — keeps the
/// FR-009-17 section names so the output shape holds even unseeded.
const BUILTIN_FALLBACK_PROMPT: &str = "Resuma a reunião em Markdown com as seções: \
## Resumo, ## Decisões, ## Próximos passos (- [ ] Tarefa — Responsável — Prazo), \
## Pontos em aberto, ## Tópicos discutidos (- Tópico (mm:ss)). \
Não invente responsáveis nem prazos; escreva no idioma predominante da reunião.";

impl Worker {
    /// Run the summary call(s) and persist `summary_md` + `summary_status`.
    /// Never writes `notes` (FR-009-19 / AC-009-06).
    pub(crate) fn summarize(&mut self, meeting: &Meeting) -> SummaryOutcome {
        let settings = get_settings(&self.app);
        let api_key =
            crate::secrets::provider_api_key(&self.app, &settings.post_process_provider_id);
        // FR-009-21: gate before any network call — the meeting is `ready`
        // without a summary and a "configure a provider" warning.
        if !summary_status(&settings, api_key.as_deref()).enabled {
            self.persist_summary(&meeting.id, None, "disabled");
            return SummaryOutcome::Disabled;
        }

        let conn = match self.conn() {
            Ok(conn) => conn,
            Err(e) => return SummaryOutcome::Failed(e),
        };
        let segments = match SqliteMeetingSegmentRepository::new(conn).list_by_meeting(&meeting.id)
        {
            Ok(segments) => segments,
            Err(e) => {
                return SummaryOutcome::Failed(CommandError::logged(
                    CommandErrorCode::Internal,
                    "Failed to list meeting segments",
                    e,
                ))
            }
        };
        let notes = SqliteNoteRepository::new(conn)
            .list_by_meeting(&meeting.id)
            .map(|rows| {
                rows.iter()
                    .map(|n| n.body_md.trim())
                    .filter(|body| !body.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n\n")
            })
            .unwrap_or_default();
        let transcript = format_transcript(&segments);
        if transcript.trim().is_empty() {
            self.persist_summary(&meeting.id, None, "disabled");
            return SummaryOutcome::Empty;
        }
        let template_prompt = self.resolve_template_prompt(meeting);

        let result = if transcript.chars().count() <= CONTEXT_BUDGET_CHARS {
            self.llm_call(
                LlmPurpose::Summary,
                &summary_system_prompt(&template_prompt),
                &summary_user_prompt(&notes, &transcript),
                4_096,
                Duration::from_secs(300),
            )
        } else {
            self.map_reduce_summary(&meeting.id, &template_prompt, &notes, &segments)
        };

        match result {
            Ok(text) => {
                let text = text.trim().to_string();
                if text.is_empty() {
                    self.persist_summary(&meeting.id, None, "error");
                    return SummaryOutcome::Failed(CommandError::new(
                        CommandErrorCode::Provider,
                        "The summary provider returned an empty response",
                    ));
                }
                self.persist_summary(&meeting.id, Some(&text), "ready");
                SummaryOutcome::Ready
            }
            Err(LlmError::MissingApiKey | LlmError::Offline | LlmError::Unsupported) => {
                // The gate above should have caught these — a provider can
                // still lose its key between checks; degrade, never error
                // the meeting (FR-009-21).
                self.persist_summary(&meeting.id, None, "disabled");
                SummaryOutcome::Disabled
            }
            Err(e) => {
                self.persist_summary(&meeting.id, None, "error");
                SummaryOutcome::Failed(CommandError::logged(
                    llm_error_code(&e),
                    "Failed to generate the meeting summary",
                    e,
                ))
            }
        }
    }

    /// FR-009-18: transcripts past the context budget are summarized in
    /// ~20-minute windows, then consolidated under the selected template.
    fn map_reduce_summary(
        &mut self,
        meeting_id: &str,
        template_prompt: &str,
        notes: &str,
        segments: &[MeetingSegment],
    ) -> Result<String, LlmError> {
        let chunks = split_for_map(segments, MAP_CHUNK_MS, CONTEXT_BUDGET_CHARS / 2);
        let mut partials: Vec<String> = Vec::with_capacity(chunks.len());
        for (i, chunk) in chunks.iter().enumerate() {
            let text = self.llm_call(
                LlmPurpose::Summary,
                &map_system_prompt(),
                &summary_user_prompt(notes, &format_transcript(chunk)),
                2_048,
                Duration::from_secs(180),
            )?;
            partials.push(text.trim().to_string());
            // Per-chunk progress inside the summary step's band.
            let pct = super::PCT_DIARIZE_DONE + ((i + 1) as u32 * 15 / chunks.len().max(1) as u32);
            self.emit_progress(meeting_id, super::STEP_SUMMARY, 4, pct);
        }
        // Reduce: consolidation calls must fit the budget too, so a
        // pathological meeting reduces in grouped rounds (bounded).
        for _round in 0..MAX_REDUCE_ROUNDS {
            let joined_len = partials.iter().map(|p| p.chars().count()).sum::<usize>();
            if partials.len() == 1 || joined_len <= CONTEXT_BUDGET_CHARS {
                return self.llm_call(
                    LlmPurpose::Summary,
                    &summary_system_prompt(template_prompt),
                    &reduce_user_prompt(notes, &partials),
                    4_096,
                    Duration::from_secs(300),
                );
            }
            let mut next: Vec<String> = Vec::new();
            for group in group_strings_by_chars(&partials, CONTEXT_BUDGET_CHARS / 2) {
                next.push(self.llm_call(
                    LlmPurpose::Summary,
                    &map_system_prompt(),
                    &reduce_user_prompt("", &group),
                    2_048,
                    Duration::from_secs(180),
                )?);
            }
            partials = next;
        }
        // The bound was exhausted — degrade to one final reduce attempt
        // rather than failing the meeting outright.
        self.llm_call(
            LlmPurpose::Summary,
            &summary_system_prompt(template_prompt),
            &reduce_user_prompt(notes, &partials),
            4_096,
            Duration::from_secs(300),
        )
    }

    /// One routed+retried LLM call on the Tauri async runtime.
    fn llm_call(
        &self,
        purpose: LlmPurpose,
        system: &str,
        user: &str,
        max_tokens: u32,
        timeout: Duration,
    ) -> Result<String, LlmError> {
        let app = self.app.clone();
        let request = LlmRequest {
            system: system.to_string(),
            messages: vec![LlmMessage::user(user)],
            max_tokens,
            temperature: 0.3,
            timeout,
            purpose,
        };
        tauri::async_runtime::block_on(async move {
            crate::llm::router::complete_for_purpose(&app, request).await
        })
        .map(|response| response.text)
    }

    /// The meeting's selected template, else the `is_default` row, else a
    /// built-in minimal prompt (defensive — migration 11 always seeds one).
    fn resolve_template_prompt(&mut self, meeting: &Meeting) -> String {
        let Some(conn) = self.conn_opt() else {
            return BUILTIN_FALLBACK_PROMPT.to_string();
        };
        let repo = SqliteSummaryTemplateRepository::new(conn);
        let resolved = meeting
            .template_id
            .as_deref()
            .and_then(|id| repo.get(id).ok().flatten())
            .or_else(|| repo.resolve_default().ok().flatten())
            .or_else(|| repo.get(BUILTIN_DEFAULT_TEMPLATE_ID).ok().flatten());
        resolved
            .map(|t| t.prompt)
            .unwrap_or_else(|| BUILTIN_FALLBACK_PROMPT.to_string())
    }

    fn persist_summary(&mut self, meeting_id: &str, summary: Option<&str>, status: &str) {
        if let Some(conn) = self.conn_opt() {
            if let Err(e) =
                SqliteMeetingRepository::new(conn).set_summary(meeting_id, summary, status)
            {
                log::warn!("Failed to persist meeting summary ({status}): {e}");
            }
        }
    }

    /// The command-facing error for a disabled summary (FR-009-21) —
    /// `MissingApiKey` keeps its own code so the frontend can point at the
    /// BYOK settings.
    pub(crate) fn disabled_error(&self) -> CommandError {
        let settings = get_settings(&self.app);
        let api_key =
            crate::secrets::provider_api_key(&self.app, &settings.post_process_provider_id);
        if summary_status(&settings, api_key.as_deref()).missing_api_key {
            CommandError::new(
                CommandErrorCode::MissingApiKey,
                "No API key configured for the summary provider",
            )
        } else {
            CommandError::new(
                CommandErrorCode::Provider,
                "The summary provider is not configured",
            )
        }
    }

    /// Suggest a title only while the row carries the `default_title`
    /// placeholder — a user-edited title is never clobbered (FR-009-20).
    /// Every failure is non-fatal: the placeholder already reads
    /// `"<App> · <data>"`, which is the documented fallback.
    pub(crate) fn suggest_title(&mut self, meeting: &Meeting) {
        if !title_is_default(&meeting.title) {
            return;
        }
        // The title call rides on summary availability — without a provider
        // (or after a failed summary) the placeholder stays.
        let settings = get_settings(&self.app);
        let api_key =
            crate::secrets::provider_api_key(&self.app, &settings.post_process_provider_id);
        if !summary_status(&settings, api_key.as_deref()).enabled {
            return;
        }
        let Some(summary) = meeting.summary_md.clone() else {
            return;
        };
        let source: String = summary.chars().take(4_000).collect();
        let suggested = self
            .llm_call(
                LlmPurpose::Title,
                &title_system_prompt(),
                &source,
                64,
                Duration::from_secs(60),
            )
            .ok()
            .and_then(|raw| sanitize_suggested_title(&raw));
        if let Some(title) = suggested {
            if let Some(conn) = self.conn_opt() {
                if let Err(e) = SqliteMeetingRepository::new(conn).set_title(&meeting.id, &title) {
                    log::warn!("Failed to persist the suggested meeting title: {e}");
                }
            }
        }
    }
}

/// Map an LLM failure to a command code.
pub(crate) fn llm_error_code(error: &LlmError) -> CommandErrorCode {
    match error {
        LlmError::MissingApiKey => CommandErrorCode::MissingApiKey,
        _ => CommandErrorCode::Provider,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn llm_errors_map_to_stable_codes() {
        assert_eq!(
            llm_error_code(&LlmError::MissingApiKey),
            CommandErrorCode::MissingApiKey
        );
        assert_eq!(
            llm_error_code(&LlmError::Timeout),
            CommandErrorCode::Provider
        );
        assert_eq!(
            llm_error_code(&LlmError::Offline),
            CommandErrorCode::Provider
        );
    }
}
