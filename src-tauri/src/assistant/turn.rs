//! Conversation-history bounds (FR-012-15 / NFR-012-03) and the spawned-task
//! turn lifecycle behind send/retry/cancel (AC-012-05).

use std::time::Duration;
use tauri::AppHandle;

use super::panel::close_panel;
use super::state::{emit_state_locked, resolve_provider};
use super::{lock_session, AssistantMessage, AssistantPhase, AssistantSession};
use crate::commands::llm::LlmErrorKind;
use crate::commands::{CommandError, CommandErrorCode, CommandResult};
use crate::llm::cli_agent;
use crate::llm::router;
use crate::llm::types::{LlmMessage, LlmPurpose, LlmRequest, LlmResponse};
use crate::settings;

/// Default upper bound on retained conversation messages (a turn costs two —
/// the user's prompt and the assistant's answer). Overridable for tests and
/// advanced tuning via `TRANSCREVE_ASSISTANT_HISTORY_LIMIT` (FR-012-15's
/// "limite configurável").
pub(crate) const DEFAULT_HISTORY_LIMIT: usize = 40;
const HISTORY_LIMIT_ENV: &str = "TRANSCREVE_ASSISTANT_HISTORY_LIMIT";

fn history_limit() -> usize {
    std::env::var(HISTORY_LIMIT_ENV)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_HISTORY_LIMIT)
}

/// Upper bound on a rendered assistant answer — the same bound the CLI-agent
/// providers enforce on their stdout (NFR-012-03). HTTP providers get the cap
/// applied here so the panel never has to layout an unbounded blob.
pub(crate) const MAX_RESPONSE_CHARS: usize = cli_agent::MAX_STDOUT_CHARS;
pub(crate) const TRUNCATED_MARKER: &str = "[response truncated]";

pub(crate) fn cap_response(text: String) -> String {
    if text.chars().count() <= MAX_RESPONSE_CHARS {
        return text;
    }
    let mut capped: String = text.chars().take(MAX_RESPONSE_CHARS).collect();
    capped.push_str("\n\n");
    capped.push_str(TRUNCATED_MARKER);
    capped
}

/// Soft character budget for the request payload — matches
/// `LlmRequest::input_chars` accounting (system prompt + message bodies).
/// ~64k chars ≈ 16k tokens, generous for a voice-assistant exchange while
/// keeping every turn inside a sane context window.
pub(crate) const MAX_INPUT_CHARS: usize = 64_000;

/// Turns sent to the provider and kept in the panel. FR-012-15: bounded
/// history — the oldest messages drop first, count-capped and
/// char-budgeted, and the retained tail always re-opens on a `user` turn
/// (Anthropic rejects assistant-first histories).
pub(crate) fn truncate_history(messages: &mut Vec<AssistantMessage>) {
    let limit = history_limit();
    if messages.len() > limit {
        let overflow = messages.len() - limit;
        messages.drain(..overflow);
    }
    drop_leading_non_user(messages);
    // Char budget — drop leading pairs until the request fits. `len > 1`
    // keeps the newest user prompt even if it alone overflows the budget.
    while messages.len() > 1
        && SYSTEM_PROMPT.chars().count() + history_chars(messages) > MAX_INPUT_CHARS
    {
        messages.remove(0);
        drop_leading_non_user(messages);
    }
}

/// Drop messages until the first remaining one is a user turn — a truncated
/// tail can open on a dangling assistant answer, which providers (Anthropic
/// among them) reject.
fn drop_leading_non_user(messages: &mut Vec<AssistantMessage>) {
    let leading = messages
        .iter()
        .position(|m| m.role == "user")
        .unwrap_or(messages.len());
    if leading > 0 {
        messages.drain(..leading);
    }
}

pub(crate) fn history_chars(messages: &[AssistantMessage]) -> usize {
    messages.iter().map(|m| m.content.chars().count()).sum()
}
// ---------------------------------------------------------------------------
// Turns
// ---------------------------------------------------------------------------

pub(crate) const SYSTEM_PROMPT: &str =
    "You are the Transcreve.ai voice assistant — a concise, helpful \
assistant the user talks to from a floating panel, often by speech. \
Keep answers short unless detail is asked for, use Markdown sparingly, \
and reply in the language the user writes or dictates in.";

/// FR-012-05 / AC-012-05: the assistant's per-call budget. CLI agents can
/// still narrow it via `timeout_secs`; the LLM router's own default is
/// longer because transcription post-processing tolerates slow batches, a
/// voice assistant does not.
const ASSISTANT_TURN_TIMEOUT: Duration = Duration::from_secs(60);

fn short_error(err: &impl std::fmt::Display) -> String {
    err.to_string().chars().take(160).collect()
}

/// Spawn the provider call for `history` and store its handle in `session`.
/// Callers must hold the session lock: `run_turn` can only commit after the
/// guard drops, and by then `in_flight` already holds its handle. Storing
/// after the spawn (two lock acquisitions) lets a fast failure clear
/// `in_flight` before the store lands — leaving a dead handle that makes
/// every later send return Busy.
/// The result commits only while the generation still matches — a cancel or
/// a new conversation bumps it, dropping stale/partial answers (AC-012-05).
fn spawn_turn_locked(
    app: &AppHandle,
    session: &mut AssistantSession,
    history: Vec<AssistantMessage>,
    generation: u64,
) {
    session.in_flight = Some(tauri::async_runtime::spawn(run_turn(
        app.clone(),
        history,
        generation,
    )));
}

pub(crate) async fn run_turn(app: AppHandle, history: Vec<AssistantMessage>, generation: u64) {
    let outcome = complete_turn(&app, &history).await;
    let Some(mut session) = lock_session(&app) else {
        return;
    };
    if session.generation != generation {
        // Cancelled or superseded — never surface a stale answer.
        return;
    }
    session.in_flight = None;
    match outcome {
        Ok(response) => {
            session
                .messages
                .push(AssistantMessage::assistant(response.text));
            truncate_history(&mut session.messages);
            session.phase = AssistantPhase::Idle;
            session.error_kind = None;
            session.error_detail = None;
        }
        Err(e) => {
            log::warn!("assistant turn failed: {e}");
            session.phase = AssistantPhase::Error;
            session.error_kind = Some(LlmErrorKind::from(&e));
            session.error_detail = Some(short_error(&e));
        }
    }
    // Emit while the guard is held so the outcome snapshot is serialized
    // against every other state event — no stale `thinking` can land after
    // it.
    emit_state_locked(&app, &mut session);
}

/// Provider call for one turn: resolve → route → complete with the narrow
/// transient-retry policy. `cli_agent/*` rides `resolve_route`'s keyless path
/// (NFR-012-02 stays inside `CliAgentProvider`: argv, no shell, kill, cap).
async fn complete_turn(
    app: &AppHandle,
    history: &[AssistantMessage],
) -> Result<LlmResponse, crate::llm::types::LlmError> {
    use crate::llm::types::LlmError;
    let settings = settings::get_settings(app);
    let provider = resolve_provider(app, &settings)
        .ok_or_else(|| LlmError::Provider("no assistant provider configured".to_string()))?;
    let api_key = crate::secrets::provider_api_key(app, &provider.id);
    let mut messages: Vec<LlmMessage> = Vec::with_capacity(history.len());
    for m in history {
        if let Some(msg) = m.to_llm() {
            messages.push(msg);
        }
    }
    let req = LlmRequest {
        system: SYSTEM_PROMPT.to_string(),
        messages,
        max_tokens: 2048,
        temperature: 0.4,
        timeout: ASSISTANT_TURN_TIMEOUT,
        purpose: LlmPurpose::Assistant,
    };
    let input_chars = req.input_chars();
    let route = router::resolve_route_for_provider(
        &settings,
        provider,
        api_key,
        LlmPurpose::Assistant,
        input_chars,
    )?;
    let llm = router::build_provider(&route)?;
    let response = if route.cli_agent.is_some() {
        // Retrying a CLI agent re-spawns the whole subscription call — a
        // timeout already burned ~60 s of wall time, so one attempt and the
        // failure surfaces (AC-012-05).
        llm.complete(req).await
    } else {
        router::complete_with_retry(llm.as_ref(), &req).await
    }?;
    Ok(LlmResponse {
        text: cap_response(response.text),
        ..response
    })
}

/// `assistant_send` — push the user message and kick the provider call.
/// FR-012-13: sending is always an explicit action; nothing is sent before.
pub fn send(app: &AppHandle, prompt: &str) -> CommandResult<()> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err(CommandError::new(
            CommandErrorCode::InvalidInput,
            "Empty prompt",
        ));
    }
    {
        let Some(mut session) = lock_session(app) else {
            return Err(CommandError::new(
                CommandErrorCode::Internal,
                "Assistant is not initialized",
            ));
        };
        if session.in_flight.is_some() {
            return Err(CommandError::new(
                CommandErrorCode::Busy,
                "The assistant is still answering",
            ));
        }
        session.phase = AssistantPhase::Thinking;
        session.error_kind = None;
        session.error_detail = None;
        session.messages.push(AssistantMessage::user(prompt));
        truncate_history(&mut session.messages);
        session.generation += 1;
        let generation = session.generation;
        let history = session.messages.clone();
        // Spawn + store + emit under this one guard — see spawn_turn_locked
        // for why the handle must land before the lock releases.
        spawn_turn_locked(app, &mut session, history, generation);
        emit_state_locked(app, &mut session);
    }
    Ok(())
}

/// `assistant_retry` — re-run the last call after a failure or a cancel. The
/// unanswered user message stays last in history, so the retry is just a
/// respawn (FR-012-14: "Tentar novamente" without retyping).
pub fn retry(app: &AppHandle) -> CommandResult<()> {
    {
        let Some(mut session) = lock_session(app) else {
            return Err(CommandError::new(
                CommandErrorCode::Internal,
                "Assistant is not initialized",
            ));
        };
        if session.in_flight.is_some() {
            return Err(CommandError::new(
                CommandErrorCode::Busy,
                "The assistant is still answering",
            ));
        }
        let retriable = matches!(
            session.phase,
            AssistantPhase::Error | AssistantPhase::Cancelled
        );
        if !retriable || session.messages.last().map(|m| m.role.as_str()) != Some("user") {
            return Err(CommandError::new(
                CommandErrorCode::InvalidInput,
                "Nothing to retry",
            ));
        }
        session.phase = AssistantPhase::Thinking;
        session.error_kind = None;
        session.error_detail = None;
        session.generation += 1;
        let generation = session.generation;
        let history = session.messages.clone();
        spawn_turn_locked(app, &mut session, history, generation);
        emit_state_locked(app, &mut session);
    }
    Ok(())
}

/// Abort the in-flight call. The JoinHandle abort drops the provider future;
/// for `cli_agent/*` the `kill_on_drop` child dies with it (AC-012-05).
pub fn cancel_in_flight(app: &AppHandle) -> bool {
    let Some(mut session) = lock_session(app) else {
        return false;
    };
    let Some(handle) = session.in_flight.take() else {
        return false;
    };
    handle.abort();
    session.generation += 1;
    session.phase = AssistantPhase::Cancelled;
    emit_state_locked(app, &mut session);
    true
}

/// Esc from the panel: while thinking it cancels (panel stays open with the
/// "cancelled" state); otherwise it closes the panel.
pub fn dismiss(app: &AppHandle) {
    if !cancel_in_flight(app) {
        close_panel(app);
    }
}

/// "Nova conversa" — abort any in-flight call and clear history (FR-012-15).
pub fn new_conversation(app: &AppHandle) {
    let Some(mut session) = lock_session(app) else {
        return;
    };
    if let Some(handle) = session.in_flight.take() {
        handle.abort();
    }
    session.generation += 1;
    session.messages.clear();
    session.phase = AssistantPhase::Idle;
    session.error_kind = None;
    session.error_detail = None;
    emit_state_locked(app, &mut session);
}
