//! `LlmProvider` over the inherited OpenAI-compatible transport:
//! `POST {base_url}/chat/completions` with a `Bearer` key (contracts §4 —
//! covers OpenAI, Groq, OpenRouter, Ollama/LM Studio and custom endpoints).

use super::http::{send, Request};
use super::provider::LlmProvider;
use super::types::{LlmError, LlmRequest, LlmResponse, LlmRole, LlmUsage};
use crate::settings::PostProcessProvider;
use crate::stt::types::{HealthReport, ProviderId};
use serde_json::{json, Value};
use std::time::Duration;

/// Health checks stay cheap and short — they only validate the credential
/// and connectivity, not the model's output.
const HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(10);

/// OpenAI's newer chat models reject the legacy `max_tokens` parameter and
/// require `max_completion_tokens`; the OpenAI-compatible ecosystem
/// (Groq, Ollama, vLLM, OpenRouter, …) still speaks `max_tokens`. Pick per
/// provider id — sending both is rejected upstream.
fn max_tokens_field(provider_id: &str) -> &'static str {
    if provider_id == "openai" {
        "max_completion_tokens"
    } else {
        "max_tokens"
    }
}

/// `POST {base_url}/chat/completions`.
pub struct OpenAiCompatibleProvider {
    /// Backs `LlmProvider::id` — read once callers land (T-067).
    #[allow(dead_code)]
    id: ProviderId,
    config: PostProcessProvider,
    /// BYOK key pulled from the OS vault; empty for keyless local endpoints.
    /// Never logged — `LlmRoute`'s Debug is redacted and this field is only
    /// consumed when building request headers.
    api_key: String,
    model: String,
}

impl OpenAiCompatibleProvider {
    pub fn new(config: PostProcessProvider, api_key: String, model: String) -> Self {
        // Gemini's native model catalog returns `models/gemini-…`, while its
        // OpenAI compatibility API expects the bare ID. Keep namespaces on
        // other endpoints, where the prefix may be meaningful.
        let google_compat = reqwest::Url::parse(&config.base_url)
            .map(|url| {
                url.scheme() == "https"
                    && url.host_str() == Some("generativelanguage.googleapis.com")
                    && url.path().trim_end_matches('/') == "/v1beta/openai"
            })
            .unwrap_or(false);
        let model = if google_compat {
            model
                .trim()
                .strip_prefix("models/")
                .unwrap_or(model.trim())
                .to_string()
        } else {
            model
        };
        Self {
            id: ProviderId::from(config.id.as_str()),
            config,
            api_key,
            model,
        }
    }

    fn chat_url(&self) -> String {
        format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        )
    }

    /// Request body: system prompt becomes the first `system` message;
    /// `stream:false` always (the pipeline expects a single response).
    fn request_body(&self, req: &LlmRequest) -> Value {
        let mut messages = Vec::with_capacity(req.messages.len() + 1);
        if !req.system.trim().is_empty() {
            messages.push(json!({ "role": "system", "content": req.system }));
        }
        for message in &req.messages {
            // `LlmRole::System` entries merge into the leading system
            // message stream — legal on OpenAI-compatible APIs.
            messages.push(json!({
                "role": message.role.as_str(),
                "content": message.content,
            }));
        }

        json!({
            "model": self.model,
            "messages": messages,
            "temperature": req.temperature,
            "stream": false,
            max_tokens_field(&self.config.id): req.max_tokens,
        })
    }
}

/// `choices[0].message.content` + `usage.{prompt,completion}_tokens`.
fn parse_chat_response(body: &[u8]) -> Result<(String, LlmUsage), LlmError> {
    let parsed: Value = serde_json::from_slice(body)
        .map_err(|_| LlmError::Provider("malformed chat completion response".to_string()))?;

    let usage = LlmUsage {
        input_tokens: parsed
            .get("usage")
            .and_then(|u| u.get("prompt_tokens"))
            .and_then(Value::as_u64),
        output_tokens: parsed
            .get("usage")
            .and_then(|u| u.get("completion_tokens"))
            .and_then(Value::as_u64),
    };

    let text = parsed
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    if text.is_empty() {
        return Err(LlmError::Provider(
            "provider returned an empty completion".to_string(),
        ));
    }
    Ok((text, usage))
}

#[async_trait::async_trait]
impl LlmProvider for OpenAiCompatibleProvider {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    async fn complete(&self, req: LlmRequest) -> Result<LlmResponse, LlmError> {
        let outcome = send(
            &self.config,
            &self.api_key,
            Request::Post(self.request_body(&req)),
            &self.chat_url(),
            req.timeout,
        )
        .await?;

        let (text, usage) = parse_chat_response(&outcome.body)?;
        Ok(LlmResponse {
            text,
            model: self.model.clone(),
            usage,
            provider_latency_ms: outcome.latency_ms,
        })
    }

    async fn health_check(&self) -> Result<HealthReport, LlmError> {
        // `GET {base}/models` is the cheapest credential check when the
        // endpoint exposes it; a 1-token completion is the fallback.
        if let Some(endpoint) = &self.config.models_endpoint {
            let url = format!("{}{}", self.config.base_url.trim_end_matches('/'), endpoint);
            let outcome = send(
                &self.config,
                &self.api_key,
                Request::Get,
                &url,
                HEALTH_CHECK_TIMEOUT,
            )
            .await?;
            return Ok(HealthReport {
                ok: outcome.status.is_success(),
                latency_ms: Some(outcome.latency_ms as u64),
                detail: None,
            });
        }

        if self.model.trim().is_empty() {
            return Err(LlmError::Provider(
                "no model configured — pick a model to test the connection".to_string(),
            ));
        }
        let probe = LlmRequest {
            system: String::new(),
            messages: vec![super::types::LlmMessage {
                role: LlmRole::User,
                content: "ping".to_string(),
            }],
            max_tokens: 1,
            temperature: 0.0,
            timeout: HEALTH_CHECK_TIMEOUT,
            purpose: super::types::LlmPurpose::Summary,
        };
        let response = self.complete(probe).await?;
        Ok(HealthReport {
            ok: true,
            latency_ms: Some(response.provider_latency_ms as u64),
            detail: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn google_compatibility_uses_the_bare_gemini_model_id() {
        let mut config = crate::settings::AppSettings::default()
            .post_process_provider("custom")
            .unwrap()
            .clone();
        config.base_url = "https://generativelanguage.googleapis.com/v1beta/openai/".to_string();
        let provider = OpenAiCompatibleProvider::new(
            config,
            String::new(),
            "models/gemini-2.5-flash".to_string(),
        );
        let request = LlmRequest {
            system: String::new(),
            messages: Vec::new(),
            max_tokens: 1024,
            temperature: 0.0,
            timeout: Duration::from_secs(30),
            purpose: super::super::types::LlmPurpose::Summary,
        };
        assert_eq!(provider.request_body(&request)["model"], "gemini-2.5-flash");
    }

    #[test]
    fn other_hosts_and_google_native_paths_preserve_model_namespaces() {
        for base_url in [
            "https://openrouter.ai/api/v1",
            "https://generativelanguage.googleapis.com/v1beta",
            "https://generativelanguage.googleapis.com.example.org/v1beta/openai/",
            "http://localhost:1234/v1",
        ] {
            let mut config = crate::settings::AppSettings::default()
                .post_process_provider("custom")
                .unwrap()
                .clone();
            config.base_url = base_url.to_string();
            let provider = OpenAiCompatibleProvider::new(
                config,
                String::new(),
                "models/gemini-2.5-flash".to_string(),
            );
            assert_eq!(provider.model, "models/gemini-2.5-flash");
        }
    }
}
