//! Cost-aware routing for `LlmProvider` calls — the `cost-aware-llm-pipeline`
//! skill applied to this codebase:
//!
//! - **Model routing by task** (`select_model`): every purpose starts on the
//!   configured (cheap) model; only `Summary` may escalate to an optional
//!   stronger model, and only when the input crosses
//!   [`SUMMARY_ESCALATION_CHARS`] — cleanup/command/title are small, latency-
//!   sensitive jobs that stay on the cheap tier.
//! - **Narrow retry** (`complete_with_policy`): only transient failures
//!   ([`LlmError::is_transient`]) retry, with exponential backoff honoring
//!   `Retry-After`. Auth/config errors fail fast — retrying them just burns
//!   budget.
//! - **Cost signal**: [`LlmResponse::usage`] carries token counts for the
//!   caller to aggregate (a priced catalog is v1.1+).
//! - **Prompt caching**: implemented inside [`super::anthropic`] for long
//!   system prompts.
//!
//! In v1 the whole surface serves a single consumer — the meeting summary
//! (T-067). The provider/model come from the BYOK post-processing settings
//! (`post_process_provider_id` / `post_process_models`); there is no separate
//! LLM account.

use super::anthropic::AnthropicProvider;
use super::openai_compat::OpenAiCompatibleProvider;
use super::provider::LlmProvider;
use super::types::{LlmError, LlmPurpose, LlmRequest, LlmResponse};
use crate::settings::{AppSettings, PostProcessProvider, APPLE_INTELLIGENCE_PROVIDER_ID};
use std::fmt;
use std::time::Duration;
use tauri::AppHandle;

/// Summary inputs above this many characters escalate to the stronger model
/// when one is configured (`settings.llm_escalation_model`). Sized like the
/// skill's `_SONNET_TEXT_THRESHOLD` — ~10k chars ≈ a 20–30 min pt-BR
/// transcript segment.
#[allow(dead_code)] // consumed by `complete_for_purpose` (T-067 callers)
pub const SUMMARY_ESCALATION_CHARS: usize = 10_000;

/// Model chosen for a call after routing. `escalated` tells the caller (and
/// the logs) that the input was large enough to leave the cheap tier.
#[allow(dead_code)] // consumed by `resolve_route` (T-067 callers)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteDecision {
    pub model: String,
    pub escalated: bool,
}

/// Input-size threshold per purpose. Only `Summary` escalates — the other
/// purposes are short, latency-bound jobs where a stronger model buys nothing
/// (`cost-aware-llm-pipeline`: "start with the cheapest model").
#[allow(dead_code)]
fn escalation_threshold(purpose: LlmPurpose) -> usize {
    match purpose {
        LlmPurpose::Summary => SUMMARY_ESCALATION_CHARS,
        LlmPurpose::Cleanup | LlmPurpose::Command | LlmPurpose::Title => usize::MAX,
    }
}

/// Cheap-first model selection. The configured model is always honored when
/// no escalation model is set; escalation only triggers for `Summary` inputs
/// past the threshold, and never to a model equal to the configured one.
#[allow(dead_code)] // pipeline-facing — called by `resolve_route` (T-067)
pub fn select_model(
    purpose: LlmPurpose,
    input_chars: usize,
    configured: &str,
    escalation: Option<&str>,
) -> RouteDecision {
    let configured = configured.trim().to_string();
    let escalation = escalation
        .map(str::trim)
        .filter(|m| !m.is_empty() && *m != configured);
    match escalation {
        Some(model) if input_chars >= escalation_threshold(purpose) => {
            log::info!(
                "LLM routing: escalating {} input ({input_chars} chars) to '{model}'",
                purpose.as_str()
            );
            RouteDecision {
                model: model.to_string(),
                escalated: true,
            }
        }
        _ => RouteDecision {
            model: configured,
            escalated: false,
        },
    }
}

/// Whether this provider needs a BYOK key at all. `custom` covers self-hosted
/// endpoints (Ollama, LM Studio) that typically have none — same rule the
/// model-list command already applies. Apple Intelligence is a native API.
pub fn requires_api_key(provider: &PostProcessProvider) -> bool {
    provider.id != "custom" && provider.id != APPLE_INTELLIGENCE_PROVIDER_ID
}

/// A fully routed call: provider config, chosen model and the vault key.
/// `Debug` is manual so a `Debug`/`{:?}` slip can never print the key.
pub struct LlmRoute {
    pub provider: PostProcessProvider,
    pub model: String,
    pub api_key: String,
    pub escalated: bool,
}

impl fmt::Debug for LlmRoute {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LlmRoute")
            .field("provider_id", &self.provider.id)
            .field("model", &self.model)
            .field("api_key", &"[REDACTED]")
            .field("escalated", &self.escalated)
            .finish()
    }
}

/// Resolve the route for a purpose against settings + a key already read from
/// the vault. Pure w.r.t. IO so it is table-testable; `complete_for_purpose`
/// is the AppHandle-facing wrapper.
///
/// Order of checks: offline gate first (FR-011-07 — no network at all), then
/// provider, key (FR-009-21), model.
#[allow(dead_code)] // called by `complete_for_purpose` (T-067 callers)
pub fn resolve_route(
    settings: &AppSettings,
    api_key: Option<String>,
    purpose: LlmPurpose,
    input_chars: usize,
) -> Result<LlmRoute, LlmError> {
    if settings.offline_mode {
        return Err(LlmError::Offline);
    }
    let provider = settings
        .active_post_process_provider()
        .cloned()
        .ok_or_else(|| LlmError::Provider("no LLM provider configured".to_string()))?;
    if provider.id == APPLE_INTELLIGENCE_PROVIDER_ID {
        return Err(LlmError::Unsupported);
    }
    let api_key = api_key.unwrap_or_default();
    if requires_api_key(&provider) && api_key.trim().is_empty() {
        return Err(LlmError::MissingApiKey);
    }
    let configured = settings
        .post_process_models
        .get(&provider.id)
        .map(String::as_str)
        .unwrap_or_default();
    let decision = select_model(
        purpose,
        input_chars,
        configured,
        settings.llm_escalation_model.as_deref(),
    );
    if decision.model.is_empty() {
        return Err(LlmError::Provider(format!(
            "no model configured for provider '{}'",
            provider.id
        )));
    }
    Ok(LlmRoute {
        provider,
        model: decision.model,
        api_key,
        escalated: decision.escalated,
    })
}

/// Build the `LlmProvider` impl for a routed call. Anything that is not
/// `anthropic` speaks the OpenAI-compatible wire format (contracts §4);
/// Apple Intelligence is native-only and rejected in `resolve_route` already.
pub fn build_provider(route: &LlmRoute) -> Result<Box<dyn LlmProvider>, LlmError> {
    if route.provider.id == APPLE_INTELLIGENCE_PROVIDER_ID {
        return Err(LlmError::Unsupported);
    }
    if route.provider.id == "anthropic" {
        Ok(Box::new(AnthropicProvider::new(
            route.provider.clone(),
            route.api_key.clone(),
            route.model.clone(),
        )))
    } else {
        Ok(Box::new(OpenAiCompatibleProvider::new(
            route.provider.clone(),
            route.api_key.clone(),
            route.model.clone(),
        )))
    }
}

/// Bounded retry policy (`cost-aware-llm-pipeline` §3). Only
/// [`LlmError::is_transient`] errors consume attempts; the wait is the
/// provider's `Retry-After` when present, else exponential backoff.
#[allow(dead_code)] // constructed by callers wanting non-default retry tuning
#[derive(Clone, Copy, Debug)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            // Post-processing is not latency-critical like dictation, so a
            // short retry ladder is affordable; still capped to keep the
            // meeting pipeline moving.
            max_attempts: 3,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(8),
        }
    }
}

/// `complete` with narrow retry — see the module docs. `req` is cheap to
/// clone per attempt.
#[allow(dead_code)] // exercised by wire tests; callers land with T-067
pub async fn complete_with_policy(
    provider: &dyn LlmProvider,
    req: &LlmRequest,
    policy: RetryPolicy,
) -> Result<LlmResponse, LlmError> {
    let max_attempts = policy.max_attempts.max(1);
    let mut delay = policy.base_delay;
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        match provider.complete(req.clone()).await {
            Ok(response) => return Ok(response),
            Err(e) if e.is_transient() && attempt < max_attempts => {
                let wait = e.retry_after().unwrap_or(delay).min(policy.max_delay);
                log::warn!(
                    "LLM {} attempt {attempt}/{max_attempts} failed transiently ({e}); retrying in {} ms",
                    req.purpose.as_str(),
                    wait.as_millis()
                );
                tokio::time::sleep(wait).await;
                delay = (delay.saturating_mul(2)).min(policy.max_delay);
            }
            Err(e) => return Err(e),
        }
    }
}

#[allow(dead_code)] // callers land with T-067
pub async fn complete_with_retry(
    provider: &dyn LlmProvider,
    req: &LlmRequest,
) -> Result<LlmResponse, LlmError> {
    complete_with_policy(provider, req, RetryPolicy::default()).await
}

/// The meeting-summary entry point (T-067): resolve provider/model/key from
/// settings + the OS vault, then call with retry. `api_key` is read via
/// `secrets::provider_api_key` — it never touches settings or logs.
#[allow(dead_code)] // the entry point T-067 calls; no v1 caller before it
pub async fn complete_for_purpose(
    app: &AppHandle,
    req: LlmRequest,
) -> Result<LlmResponse, LlmError> {
    let settings = crate::settings::get_settings(app);
    let api_key = crate::secrets::provider_api_key(app, &settings.post_process_provider_id);
    let route = resolve_route(&settings, api_key, req.purpose, req.input_chars())?;
    let provider = build_provider(&route)?;
    complete_with_retry(provider.as_ref(), &req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(id: &str) -> PostProcessProvider {
        PostProcessProvider {
            id: id.to_string(),
            label: id.to_string(),
            base_url: format!("https://example.com/{id}"),
            allow_base_url_edit: true,
            models_endpoint: Some("/models".to_string()),
            supports_structured_output: false,
        }
    }

    fn settings_with(provider_id: &str, model: &str) -> AppSettings {
        let mut settings = AppSettings {
            post_process_provider_id: provider_id.to_string(),
            ..Default::default()
        };
        settings
            .post_process_models
            .insert(provider_id.to_string(), model.to_string());
        settings
    }

    #[test]
    fn configured_model_is_always_the_cheap_default() {
        let decision = select_model(LlmPurpose::Summary, 500, "haiku", Some("sonnet"));
        assert_eq!(
            decision,
            RouteDecision {
                model: "haiku".to_string(),
                escalated: false
            }
        );
        // No escalation configured → configured model, whatever the size.
        let decision = select_model(LlmPurpose::Summary, 50_000, "haiku", None);
        assert!(!decision.escalated);
        assert_eq!(decision.model, "haiku");
    }

    #[test]
    fn summary_escalates_only_past_the_threshold() {
        let small = select_model(LlmPurpose::Summary, 9_999, "haiku", Some("sonnet"));
        assert!(!small.escalated);
        let big = select_model(LlmPurpose::Summary, 10_000, "haiku", Some("sonnet"));
        assert_eq!(big.model, "sonnet");
        assert!(big.escalated);
        // Escalating to the same model is a no-op.
        let same = select_model(LlmPurpose::Summary, 50_000, "haiku", Some("haiku"));
        assert!(!same.escalated);
    }

    #[test]
    fn latency_bound_purposes_never_escalate() {
        for purpose in [LlmPurpose::Cleanup, LlmPurpose::Command, LlmPurpose::Title] {
            let decision = select_model(purpose, 10_000_000, "cheap", Some("strong"));
            assert_eq!(decision.model, "cheap");
            assert!(!decision.escalated);
        }
    }

    #[test]
    fn resolve_route_gates_on_offline_provider_key_model() {
        let mut settings = settings_with("openai", "gpt-4o-mini");

        // Offline mode blocks everything (FR-011-07).
        settings.offline_mode = true;
        assert!(matches!(
            resolve_route(&settings, Some("sk-x".into()), LlmPurpose::Summary, 10),
            Err(LlmError::Offline)
        ));
        settings.offline_mode = false;

        // BYOK: no key → MissingApiKey (FR-009-21).
        assert!(matches!(
            resolve_route(&settings, None, LlmPurpose::Summary, 10),
            Err(LlmError::MissingApiKey)
        ));
        assert!(matches!(
            resolve_route(&settings, Some("  ".into()), LlmPurpose::Summary, 10),
            Err(LlmError::MissingApiKey)
        ));

        // With a key, the configured model routes through.
        let route =
            resolve_route(&settings, Some("sk-x".into()), LlmPurpose::Summary, 10).expect("route");
        assert_eq!(route.model, "gpt-4o-mini");
        assert!(!route.escalated);

        // No model configured → configuration error, not a transient one.
        settings
            .post_process_models
            .insert("openai".to_string(), String::new());
        match resolve_route(&settings, Some("sk-x".into()), LlmPurpose::Summary, 10) {
            Err(LlmError::Provider(msg)) => assert!(msg.contains("no model")),
            other => panic!("expected Provider(no model), got {other:?}"),
        }
    }

    #[test]
    fn custom_provider_needs_no_key() {
        let settings = settings_with("custom", "llama3");
        assert!(!requires_api_key(
            settings.post_process_provider("custom").unwrap()
        ));
        let route = resolve_route(&settings, None, LlmPurpose::Summary, 10).expect("route");
        assert_eq!(route.provider.id, "custom");
    }

    #[test]
    fn apple_intelligence_is_not_an_http_llm() {
        let mut settings = settings_with("openai", "gpt-4o-mini");
        settings.post_process_provider_id = APPLE_INTELLIGENCE_PROVIDER_ID.to_string();
        settings
            .post_process_providers
            .push(provider(APPLE_INTELLIGENCE_PROVIDER_ID));
        assert!(matches!(
            resolve_route(&settings, None, LlmPurpose::Summary, 10),
            Err(LlmError::Unsupported)
        ));
    }

    #[test]
    fn route_debug_never_prints_the_key() {
        let route = LlmRoute {
            provider: provider("openai"),
            model: "m".to_string(),
            api_key: "sk-super-secret".to_string(),
            escalated: false,
        };
        let rendered = format!("{route:?}");
        assert!(!rendered.contains("sk-super-secret"));
        assert!(rendered.contains("[REDACTED]"));
    }

    #[test]
    fn escalation_setting_drives_route() {
        let mut settings = settings_with("openai", "haiku");
        settings.llm_escalation_model = Some("sonnet".to_string());
        let route = resolve_route(
            &settings,
            Some("sk-x".into()),
            LlmPurpose::Summary,
            SUMMARY_ESCALATION_CHARS,
        )
        .expect("route");
        assert_eq!(route.model, "sonnet");
        assert!(route.escalated);
    }
}
