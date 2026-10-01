//! `LlmProvider` for Anthropic's Messages API:
//! `POST {base_url}/messages` with `x-api-key` + `anthropic-version`
//! (contracts §4). The inherited `llm_client` only speaks
//! `/chat/completions`, so Anthropic gets its own wire format here — the auth
//! headers themselves are shared (`llm_client::create_client`).

use super::http::{send, Request};
use super::provider::LlmProvider;
use super::types::{LlmError, LlmRequest, LlmResponse, LlmRole, LlmUsage};
use crate::settings::PostProcessProvider;
use crate::stt::types::{HealthReport, ProviderId};
use serde_json::{json, Value};
use std::time::Duration;

const HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(10);

/// System prompts at least this long are sent as a cached content block —
/// `cache_control: ephemeral` avoids re-billing the template on every call
/// (`cost-aware-llm-pipeline` §4). ~1024 tokens ≈ 4 000 chars.
const PROMPT_CACHE_MIN_CHARS: usize = 4_000;

/// `POST {base_url}/messages` (base is `https://api.anthropic.com/v1`).
pub struct AnthropicProvider {
    /// Backs `LlmProvider::id` — read once callers land (T-067).
    #[allow(dead_code)]
    id: ProviderId,
    config: PostProcessProvider,
    /// BYOK key pulled from the OS vault; only consumed as a header value.
    api_key: String,
    model: String,
}

impl AnthropicProvider {
    pub fn new(config: PostProcessProvider, api_key: String, model: String) -> Self {
        Self {
            id: ProviderId::from(config.id.as_str()),
            config,
            api_key,
            model,
        }
    }

    fn messages_url(&self) -> String {
        format!("{}/messages", self.config.base_url.trim_end_matches('/'))
    }

    /// Anthropic's `system` is a top-level field, not a message role. Long
    /// system prompts (the meeting-summary template + rules) go out as a
    /// cached text block; `LlmRole::System` messages merge in first.
    fn system_field(&self, req: &LlmRequest) -> Option<Value> {
        let mut system = req.system.clone();
        for message in &req.messages {
            if message.role == LlmRole::System {
                if !system.is_empty() {
                    system.push_str("\n\n");
                }
                system.push_str(&message.content);
            }
        }
        let system = system.trim().to_string();
        if system.is_empty() {
            return None;
        }
        if system.chars().count() >= PROMPT_CACHE_MIN_CHARS {
            Some(json!([{
                "type": "text",
                "text": system,
                "cache_control": { "type": "ephemeral" },
            }]))
        } else {
            Some(Value::String(system))
        }
    }

    fn request_body(&self, req: &LlmRequest) -> Value {
        let messages: Vec<Value> = req
            .messages
            .iter()
            .filter(|m| m.role != LlmRole::System)
            .map(|m| {
                json!({
                    "role": m.role.as_str(),
                    "content": m.content,
                })
            })
            .collect();

        let mut body = json!({
            "model": self.model,
            "max_tokens": req.max_tokens,
            "temperature": req.temperature,
            "messages": messages,
        });
        if let Some(system) = self.system_field(req) {
            body["system"] = system;
        }
        body
    }
}

/// `content[].text` (joined) + `usage.{input,output}_tokens`.
fn parse_messages_response(body: &[u8]) -> Result<(String, LlmUsage), LlmError> {
    let parsed: Value = serde_json::from_slice(body)
        .map_err(|_| LlmError::Provider("malformed Anthropic response".to_string()))?;

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

    let text = parsed
        .get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();

    if text.is_empty() {
        return Err(LlmError::Provider(
            "provider returned an empty completion".to_string(),
        ));
    }
    Ok((text, usage))
}

#[async_trait::async_trait]
impl LlmProvider for AnthropicProvider {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    async fn complete(&self, req: LlmRequest) -> Result<LlmResponse, LlmError> {
        let outcome = send(
            &self.config,
            &self.api_key,
            Request::Post(self.request_body(&req)),
            &self.messages_url(),
            req.timeout,
        )
        .await?;

        let (text, usage) = parse_messages_response(&outcome.body)?;
        Ok(LlmResponse {
            text,
            model: self.model.clone(),
            usage,
            provider_latency_ms: outcome.latency_ms,
        })
    }

    async fn health_check(&self) -> Result<HealthReport, LlmError> {
        // `GET /v1/models` validates the key without spending tokens.
        let url = format!("{}/models", self.config.base_url.trim_end_matches('/'));
        let outcome = send(
            &self.config,
            &self.api_key,
            Request::Get,
            &url,
            HEALTH_CHECK_TIMEOUT,
        )
        .await?;
        Ok(HealthReport {
            ok: outcome.status.is_success(),
            latency_ms: Some(outcome.latency_ms as u64),
            detail: None,
        })
    }
}
