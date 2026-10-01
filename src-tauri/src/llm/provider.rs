//! Contrato `LlmProvider` — `specs/architecture/contracts.md` §3.
//!
//! Text-only completions; nothing a model returns is executed (FR-011-22).
//! Implementations live in [`super::openai_compat`] and [`super::anthropic`];
//! routing/retry live in [`super::router`].

use super::types::{LlmError, LlmRequest, LlmResponse};
use crate::stt::types::{HealthReport, ProviderId};

/// A text-completion provider (BYOK). Send + Sync so it can be boxed and
/// shared across the async runtime by the meeting pipeline.
#[async_trait::async_trait]
pub trait LlmProvider: Send + Sync {
    /// Metrics/logging label — callers land with T-067.
    #[allow(dead_code)]
    fn id(&self) -> &ProviderId;
    async fn complete(&self, req: LlmRequest) -> Result<LlmResponse, LlmError>;
    /// Validate credentials/connectivity cheaply — `GET /models` where the
    /// endpoint offers it, a 1-token completion otherwise. Backs the
    /// `test_llm_connection` command.
    async fn health_check(&self) -> Result<HealthReport, LlmError>;
}
