//! Prompt flattening (FR-012-15), response parsing and auth-probe checks
//! (NFR-012-03 — bounded output with truncation markers).

use super::spawn::sanitize_detail;
use super::{HeadlessOutput, MAX_STDOUT_CHARS};
use crate::llm::types::{LlmError, LlmRequest, LlmUsage};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Prompt flattening (FR-012-15 — CLIs are stateless per invocation)
// ---------------------------------------------------------------------------

/// Flatten `system` + chat history into a single delimited transcript the
/// CLI consumes as one prompt. Delimiters are plain tags so the model can
/// still tell roles apart; nothing here is parsed back.
pub(crate) fn flatten_prompt(req: &LlmRequest) -> String {
    let mut out = String::new();
    let system = req.system.trim();
    if !system.is_empty() {
        out.push_str("<system>\n");
        out.push_str(system);
        out.push_str("\n</system>\n\n");
    }
    for message in &req.messages {
        let role = message.role.as_str();
        out.push('<');
        out.push_str(role);
        out.push_str(">\n");
        out.push_str(message.content.trim_end());
        out.push_str("\n</");
        out.push_str(role);
        out.push_str(">\n\n");
    }
    out.trim_end().to_string()
}

// ---------------------------------------------------------------------------
// Output parsing
// ---------------------------------------------------------------------------

/// Truncate a decoded response to [`MAX_STDOUT_CHARS`], marking the cut
/// (NFR-012-03 — truncate with a notice, never overflow the caller).
pub(crate) fn cap_text(text: String) -> String {
    if text.chars().count() <= MAX_STDOUT_CHARS {
        return text;
    }
    let mut truncated: String = text.chars().take(MAX_STDOUT_CHARS).collect();
    truncated.push_str("\n\n[response truncated]");
    truncated
}

/// Append the truncation marker to an already-trimmed, non-empty answer.
pub(crate) fn with_truncation_marker(mut text: String) -> String {
    text.push_str("\n\n[response truncated]");
    text
}

/// Plain-text stdout (claude/cursor/devin non-JSON paths): trim, reject
/// empty, cap. When the byte cap was hit mid-stream the marker is appended
/// even if the char cap below was not reached.
pub(crate) fn parse_plain_text(
    stdout: &[u8],
    truncated: bool,
) -> Result<(String, LlmUsage), LlmError> {
    let text = String::from_utf8_lossy(stdout).trim().to_string();
    if text.is_empty() {
        return Err(LlmError::Provider(
            "CLI returned an empty completion".to_string(),
        ));
    }
    let text = cap_text(text);
    Ok((
        if truncated && !text.ends_with("[response truncated]") {
            with_truncation_marker(text)
        } else {
            text
        },
        LlmUsage::default(),
    ))
}

/// `codex exec --json` emits JSONL events; the answer is the concatenation
/// of `item.completed` events whose item is an `agent_message`. `turn.failed`
/// / top-level `error` events become the `LlmError::Provider` detail. When
/// nothing parses as JSONL at all the raw stdout is treated as plain text —
/// tolerance for older/newer codex builds — *unless* the byte cap cut the
/// stream: half-parsed JSONL must never surface as a "completion".
pub(crate) fn parse_codex_jsonl(
    stdout: &[u8],
    truncated: bool,
) -> Result<(String, LlmUsage), LlmError> {
    let text = String::from_utf8_lossy(stdout);
    let mut events = 0usize;
    let mut messages: Vec<String> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    let mut usage = LlmUsage::default();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        events += 1;
        match event.get("type").and_then(Value::as_str) {
            Some("item.completed") => {
                let item = event.get("item");
                if item.and_then(|i| i.get("type")).and_then(Value::as_str) == Some("agent_message")
                {
                    if let Some(t) = item.and_then(|i| i.get("text")).and_then(Value::as_str) {
                        messages.push(t.to_string());
                    }
                }
            }
            Some("turn.completed") => {
                usage = LlmUsage {
                    input_tokens: event
                        .get("usage")
                        .and_then(|u| u.get("input_tokens"))
                        .and_then(Value::as_u64),
                    output_tokens: event
                        .get("usage")
                        .and_then(|u| u.get("output_tokens"))
                        .and_then(Value::as_u64),
                };
            }
            Some("turn.failed") => {
                if let Some(msg) = event
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(Value::as_str)
                {
                    failures.push(msg.to_string());
                }
            }
            Some("error") => {
                if let Some(msg) = event.get("message").and_then(Value::as_str) {
                    failures.push(msg.to_string());
                }
            }
            _ => {}
        }
    }

    if events == 0 {
        return if truncated {
            Err(LlmError::Provider(
                "codex output was truncated mid-stream and could not be parsed".to_string(),
            ))
        } else {
            parse_plain_text(stdout, false)
        };
    }
    if !messages.is_empty() {
        let text = cap_text(messages.join("\n"));
        return Ok((
            if truncated {
                with_truncation_marker(text)
            } else {
                text
            },
            usage,
        ));
    }
    if let Some(msg) = failures.first() {
        return Err(LlmError::Provider(sanitize_detail(msg)));
    }
    Err(LlmError::Provider(
        "codex produced no agent message".to_string(),
    ))
}

/// `claude -p --output-format json` returns one result object:
/// `{type:"result", is_error, result, usage:{input_tokens,output_tokens,…}}`.
/// Anything that does not parse as that object is treated as plain text —
/// unless the byte cap cut the stream, in which case unparseable output is an
/// error, never a partially-parsed "completion".
pub(crate) fn parse_claude_json(
    stdout: &[u8],
    truncated: bool,
) -> Result<(String, LlmUsage), LlmError> {
    let text = String::from_utf8_lossy(stdout);
    let trimmed = text.trim();
    let Ok(parsed) = serde_json::from_str::<Value>(trimmed) else {
        return if truncated {
            Err(LlmError::Provider(
                "claude output was truncated mid-stream and could not be parsed".to_string(),
            ))
        } else {
            parse_plain_text(stdout, false)
        };
    };

    if let Some(result) = parsed.get("result") {
        if parsed.get("is_error").and_then(Value::as_bool) == Some(true) {
            let detail = result.as_str().unwrap_or("claude reported an error");
            return Err(LlmError::Provider(sanitize_detail(detail)));
        }
        let answer = result.as_str().unwrap_or_default().trim().to_string();
        if answer.is_empty() {
            return Err(LlmError::Provider(
                "CLI returned an empty completion".to_string(),
            ));
        }
        let usage = LlmUsage {
            input_tokens: parsed
                .get("usage")
                .and_then(|u| u.get("input_tokens"))
                .and_then(Value::as_u64),
            output_tokens: parsed
                .get("usage")
                .and_then(|u| u.get("output_tokens"))
                .and_then(Value::as_u64),
        };
        let answer = cap_text(answer);
        return Ok((
            if truncated {
                with_truncation_marker(answer)
            } else {
                answer
            },
            usage,
        ));
    }

    if truncated {
        return Err(LlmError::Provider(
            "claude output was truncated mid-stream and could not be parsed".to_string(),
        ));
    }
    parse_plain_text(stdout, false)
}

// ---------------------------------------------------------------------------
// Auth probes (`--version` proves the binary; these prove the session)
// ---------------------------------------------------------------------------

/// Exit-code-only probe (`codex login status`): 0 means the CLI considers
/// itself logged in / usable.
pub(crate) fn check_auth_exit_code(out: &HeadlessOutput) -> Option<String> {
    if out.exit_code == Some(0) {
        None
    } else {
        Some("the CLI is not signed in or its config failed to load".to_string())
    }
}

/// `claude auth status` exits 0 even when logged out — the JSON body's
/// `loggedIn` flag is the real signal.
pub(crate) fn check_auth_claude(out: &HeadlessOutput) -> Option<String> {
    if out.exit_code != Some(0) {
        return Some("the CLI is not signed in or its config failed to load".to_string());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    match serde_json::from_str::<Value>(text.trim())
        .ok()
        .and_then(|v| v.get("loggedIn").and_then(Value::as_bool))
    {
        Some(true) => None,
        _ => Some("the CLI is not signed in — run `claude auth login`".to_string()),
    }
}
