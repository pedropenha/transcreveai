//! Tipos do contrato `LlmProvider` — `specs/architecture/contracts.md` §3.
//!
//! No tool use: the LLM only ever returns text (contracts §3, FR-011-22).
//! In v1 the only consumer is the meeting summary (T-050/T-067, BYOK); the
//! types already carry `purpose` so later tasks (cleanup, command mode,
//! titles) route through the same surface without changing the trait.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Why a completion is being requested — drives per-task model routing
/// (`cost-aware-llm-pipeline`) and metrics labels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmPurpose {
    /// Dictation cleanup (v1.1+, T-051).
    Cleanup,
    /// Command Mode (v1.1+, T-056).
    Command,
    /// Meeting summary — the only purpose wired in v1 (T-067 consumes).
    Summary,
    /// Suggested meeting/note titles.
    Title,
    /// Voice-assistant overlay turns (F012, T-091).
    Assistant,
}

impl LlmPurpose {
    /// Stable label for logs/metrics. Only consumed by the router's retry
    /// logging today — the pipeline callers land with T-067.
    #[allow(dead_code)]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Cleanup => "cleanup",
            Self::Command => "command",
            Self::Summary => "summary",
            Self::Title => "title",
            Self::Assistant => "assistant",
        }
    }
}

/// Chat roles mapped onto each provider's wire format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LlmRole {
    System,
    User,
    Assistant,
}

impl LlmRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
}

/// One chat message (contracts §3: role + content, text only).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LlmMessage {
    pub role: LlmRole,
    pub content: String,
}

impl LlmMessage {
    /// Convenience constructor for the dominant role (T-067 builds requests
    /// with it).
    #[allow(dead_code)]
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: LlmRole::User,
            content: content.into(),
        }
    }
}

/// A completion request (contracts §3). The caller owns the timeout so the
/// meeting post-processing can afford a longer window than dictation cleanup.
#[derive(Clone, Debug)]
pub struct LlmRequest {
    /// System prompt; sent as a `system` message on OpenAI-compatible APIs
    /// and as Anthropic's top-level `system` field (with prompt caching when
    /// it is long — `cost-aware-llm-pipeline` §4).
    pub system: String,
    pub messages: Vec<LlmMessage>,
    pub max_tokens: u32,
    /// 0.0–0.3 for cleanup tasks; summaries can go a bit higher.
    pub temperature: f32,
    pub timeout: Duration,
    /// Routing/metrics label — read by `router` once callers land (T-067).
    #[allow(dead_code)]
    pub purpose: LlmPurpose,
}

impl LlmRequest {
    /// Rough input size used by the cost-aware router for escalation
    /// decisions (`system` + every message body).
    #[allow(dead_code)]
    pub fn input_chars(&self) -> usize {
        self.system.chars().count()
            + self
                .messages
                .iter()
                .map(|m| m.content.chars().count())
                .sum::<usize>()
    }
}

/// Token usage reported by the provider, when present — the raw material for
/// the cost tracking of `cost-aware-llm-pipeline` §2 (a priced catalog is a
/// v1.1+ concern; consumers aggregate these counts today).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LlmUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}

/// A completion response. `text` is the only thing callers consume — nothing
/// the model returns is ever executed (contracts §3, FR-011-22).
/// `allow(dead_code)`: fields are the contract surface; readers land with the
/// meeting pipeline (T-067).
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct LlmResponse {
    pub text: String,
    /// The model that actually answered (post routing/escalation).
    pub model: String,
    pub usage: LlmUsage,
    pub provider_latency_ms: u32,
}

/// Typed provider errors (contracts §3, mirroring `SttError` §2). Variants
/// carry no secrets or raw request bodies — provider error text embedded in
/// `Provider`/`Unavailable` is already redacted by the transport layer.
#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    /// Transport failure (DNS, connect reset, TLS, body read). The wrapped
    /// `reqwest::Error` Display can include the request URL — never surface
    /// it verbatim to the UI; use a fixed message and log via
    /// `llm_client::report_reqwest_error` instead.
    #[error("network error")]
    Network(#[source] reqwest::Error),
    /// The request exceeded `LlmRequest::timeout`.
    #[error("request timed out")]
    Timeout,
    /// 401/403 — credential rejected. Never retried
    /// (`cost-aware-llm-pipeline` §3).
    #[error("invalid or unauthorized API key")]
    Auth,
    /// 429 — transient; `retry_after` comes from the `Retry-After` header.
    #[error("rate limited by the provider")]
    RateLimited { retry_after: Option<Duration> },
    /// 5xx — server-side failure, transient by definition.
    #[error("provider server error (HTTP {0})")]
    Unavailable(u16),
    /// The provider requires a BYOK key and the vault holds none
    /// (FR-009-21 — the summary stays disabled).
    #[error("no API key configured for this provider")]
    MissingApiKey,
    /// `privacy.offline_mode` blocks all outbound calls (FR-011-07).
    #[error("blocked by offline mode")]
    Offline,
    /// The configured provider is not an HTTP LLM endpoint (e.g. Apple
    /// Intelligence — native-only, not bridged to `LlmProvider` in v1).
    #[error("provider is not supported for LLM requests")]
    Unsupported,
    /// Any other non-transient provider answer (4xx, malformed body).
    /// `{0}` is a bounded, secret-redacted detail string.
    #[error("provider error: {0}")]
    Provider(String),
}

#[allow(dead_code)] // retry classification — driven by `router`'s policy (T-067 callers).
impl LlmError {
    /// Transient errors worth a bounded retry (`cost-aware-llm-pipeline` §3 —
    /// narrow retry). `Auth`, `MissingApiKey`, `Offline`, `Unsupported` and
    /// `Provider` fail fast: they are configuration or validation problems.
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            Self::Network(_) | Self::Timeout | Self::RateLimited { .. } | Self::Unavailable(_)
        )
    }

    /// Server-provided backoff hint, when the error carries one.
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::RateLimited { retry_after } => *retry_after,
            _ => None,
        }
    }
}
