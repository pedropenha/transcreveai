//! LLM BYOK surface — the `LlmProvider` contract (`contracts.md` §3–4) on top
//! of the inherited `llm_client` transport.
//!
//! * [`types`] — `LlmRequest`/`LlmResponse`/`LlmError`/`LlmPurpose`
//!   (text-only; nothing returned is ever executed, FR-011-22).
//! * [`provider`] — the `LlmProvider` trait.
//! * [`openai_compat`] — `POST {base}/chat/completions` (OpenAI, Groq,
//!   OpenRouter, Ollama, custom endpoints).
//! * [`anthropic`] — `POST {base}/messages` (`x-api-key` + prompt caching).
//! * [`router`] — cost-aware model routing + narrow retry +
//!   `complete_for_purpose`, the entry point T-067 consumes.
//! * [`cli_agent`] — `cli_agent/*` providers driving installed agent CLIs
//!   (codex, claude, cursor-agent, devin) headlessly — subscription auth,
//!   no API key, cost zero (F012, FR-012-01..05).
//!
//! v1 scope (T-050): only the meeting summary calls this (BYOK). Keys come
//! from the OS credential vault via `secrets::provider_api_key` — never from
//! settings, never into logs. With no key configured the summary is simply
//! disabled; transcription and notes never touch this module (FR-009-21).

mod anthropic;
pub mod cli_agent;
mod http;
mod openai_compat;
mod provider;
pub mod router;
pub mod types;

// Re-exports for the T-067 meeting post-processing lane — consumed through
// `crate::llm::…` paths today (`commands::llm` reaches `router`/`types`
// directly), so the lint is silenced rather than the API surface trimmed.
#[allow(unused_imports)]
pub use provider::LlmProvider;
#[allow(unused_imports)]
pub use router::{build_provider, complete_for_purpose, complete_with_retry, resolve_route};
#[allow(unused_imports)]
pub use types::{LlmError, LlmMessage, LlmPurpose, LlmRequest, LlmResponse, LlmRole, LlmUsage};

#[cfg(test)]
mod wire_tests;
