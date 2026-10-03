//! Tauri commands for the LLM BYOK surface (T-050, FR-009-21):
//!
//! * `test_llm_connection` — validates the vault key against the configured
//!   provider (`LlmProvider::health_check`). The key is read via
//!   `secrets::provider_api_key` and only ever becomes request headers; the
//!   report carries a classified `kind`, never the key.
//! * `llm_summary_status` — the gating state the meeting window needs:
//!   summary enabled ⇔ provider + model configured and a key present (or the
//!   provider needs none, e.g. a local `custom` endpoint), and offline mode
//!   off. FR-009-21 — transcription and notes never depend on this.

use crate::commands::{CommandError, CommandErrorCode, CommandResult};
use crate::llm::cli_agent;
use crate::llm::router;
use crate::llm::types::LlmError;
use crate::secrets;
use crate::settings::{self, AppSettings, CliAgentConfig, PostProcessProvider};
use serde::Serialize;
use specta::Type;
use tauri::AppHandle;

/// Stable, machine-readable failure class for `test_llm_connection` — the
/// frontend localizes on this; `detail` is only a diagnostic extra.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum LlmErrorKind {
    Network,
    Timeout,
    Auth,
    RateLimited,
    Unavailable,
    MissingApiKey,
    Offline,
    Unsupported,
    Provider,
}

impl From<&LlmError> for LlmErrorKind {
    fn from(error: &LlmError) -> Self {
        match error {
            LlmError::Network(_) => Self::Network,
            LlmError::Timeout => Self::Timeout,
            LlmError::Auth => Self::Auth,
            LlmError::RateLimited { .. } => Self::RateLimited,
            LlmError::Unavailable(_) => Self::Unavailable,
            LlmError::MissingApiKey => Self::MissingApiKey,
            LlmError::Offline => Self::Offline,
            LlmError::Unsupported => Self::Unsupported,
            LlmError::Provider(_) => Self::Provider,
        }
    }
}

/// Result of `test_llm_connection`. `detail` is a short user-facing hint;
/// secrets and raw URLs never reach it (transport errors are classified, not
/// stringified).
#[derive(Debug, Clone, Serialize, Type)]
pub struct LlmConnectionReport {
    pub ok: bool,
    pub provider_id: String,
    pub latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<LlmErrorKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl LlmConnectionReport {
    fn failure(provider_id: &str, error: &LlmError, detail: String) -> Self {
        Self {
            ok: false,
            provider_id: provider_id.to_string(),
            latency_ms: None,
            kind: Some(LlmErrorKind::from(error)),
            detail: Some(detail),
        }
    }
}

/// Short, user-facing detail per failure class — English source text; the UI
/// switches on `kind` for localization.
fn detail_for(error: &LlmError) -> String {
    match error {
        LlmError::Auth => "The provider rejected the API key".to_string(),
        LlmError::Timeout => "The provider did not answer in time".to_string(),
        LlmError::RateLimited { .. } => "The provider is rate limiting requests".to_string(),
        LlmError::Unavailable(status) => format!("The provider returned a server error ({status})"),
        LlmError::Network(_) => "Could not reach the provider".to_string(),
        LlmError::MissingApiKey => "No API key configured for this provider".to_string(),
        LlmError::Offline => "Offline mode is on".to_string(),
        LlmError::Unsupported => "This provider cannot be tested over HTTP".to_string(),
        LlmError::Provider(detail) => detail.clone(),
    }
}

/// Gating state for the meeting summary (FR-009-21). `enabled` is the single
/// flag the Resumo tab needs; the other fields explain *why* so the UI can
/// point the user at the fix.
#[derive(Debug, Clone, Serialize, Type)]
pub struct LlmSummaryStatus {
    pub enabled: bool,
    pub provider_id: Option<String>,
    pub provider_label: Option<String>,
    pub model: Option<String>,
    /// Masked key hint (`••••` + short suffix), `None` when no key is stored.
    pub key_hint: Option<String>,
    pub missing_api_key: bool,
    pub missing_model: bool,
    /// Offline mode is on — summaries need the network (FR-011-07).
    pub offline: bool,
}

/// Pure status computation — split out for tests; the command reads settings
/// and the vault and feeds both in. `pub(crate)` for the meeting
/// post-processor, which reuses it as the FR-009-21 pre-flight gate.
pub(crate) fn summary_status(settings: &AppSettings, api_key: Option<&str>) -> LlmSummaryStatus {
    let provider = settings.active_post_process_provider();
    let model = provider
        .and_then(|p| settings.post_process_models.get(&p.id))
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty());
    let needs_key = provider.map(router::requires_api_key).unwrap_or(false);
    let has_key = api_key.map(|k| !k.trim().is_empty()).unwrap_or(false);
    let missing_api_key = needs_key && !has_key;
    // `cli_agent/*` providers need no model — empty means the CLI's default —
    // but they are only usable when enabled and detected on PATH (FR-012-02).
    // Experimental adapters (no verified non-mutating mode) are never usable.
    let cli_agent_usable = provider
        .filter(|p| cli_agent::is_cli_agent(&p.id))
        .map(|p| {
            let config = settings.cli_agent_config(&p.id);
            config.enabled
                && cli_agent::adapter_for(&p.id)
                    .map(|spec| {
                        !spec.experimental && cli_agent::resolve_binary(spec, &config).is_some()
                    })
                    .unwrap_or(false)
        });
    let missing_model = model.is_none() && cli_agent_usable.is_none();
    let offline = settings.offline_mode;
    LlmSummaryStatus {
        enabled: provider.is_some()
            && !missing_api_key
            && !missing_model
            && !offline
            && cli_agent_usable.unwrap_or(true),
        provider_id: provider.map(|p| p.id.clone()),
        provider_label: provider.map(|p| p.label.clone()),
        model,
        key_hint: secrets::hint_for(api_key),
        missing_api_key,
        missing_model,
        offline,
    }
}

/// Gating query for the meeting window (FR-009-21): `enabled == false` means
/// the Resumo tab stays disabled with a hint to configure a BYOK key — while
/// transcription and "Minhas notas" keep working regardless.
#[tauri::command]
#[specta::specta]
pub async fn llm_summary_status(app: AppHandle) -> CommandResult<LlmSummaryStatus> {
    let settings = settings::get_settings(&app);
    // Key presence is what gates the summary — pull the hint path via
    // `provider_api_key` so a stranded plaintext copy still counts (T-016).
    let provider_id = settings.post_process_provider_id.clone();
    let api_key = secrets::provider_api_key(&app, &provider_id);
    Ok(summary_status(&settings, api_key.as_deref()))
}

/// Validate the configured provider's vault key with a cheap health check
/// (`GET /models` where available, else a 1-token completion). Never returns
/// the key; failures come back as `ok:false` + a classified `kind` rather
/// than a command error, since "the test failed" is a normal outcome.
#[tauri::command]
#[specta::specta]
pub async fn test_llm_connection(
    app: AppHandle,
    provider_id: String,
) -> CommandResult<LlmConnectionReport> {
    let settings = settings::get_settings(&app);
    let provider: PostProcessProvider = settings
        .post_process_provider(&provider_id)
        .cloned()
        .ok_or_else(|| {
            CommandError::new(
                CommandErrorCode::NotFound,
                format!("Provider '{provider_id}' not found"),
            )
        })?;

    if settings.offline_mode {
        return Ok(LlmConnectionReport::failure(
            &provider_id,
            &LlmError::Offline,
            detail_for(&LlmError::Offline),
        ));
    }

    let api_key = secrets::provider_api_key(&app, &provider_id).unwrap_or_default();
    if router::requires_api_key(&provider) && api_key.trim().is_empty() {
        return Ok(LlmConnectionReport::failure(
            &provider_id,
            &LlmError::MissingApiKey,
            detail_for(&LlmError::MissingApiKey),
        ));
    }

    let model = settings
        .post_process_models
        .get(&provider.id)
        .cloned()
        .unwrap_or_default();

    let llm = router::build_provider(&router::LlmRoute {
        provider: provider.clone(),
        model,
        api_key,
        escalated: false,
        cli_agent: cli_agent::is_cli_agent(&provider.id)
            .then(|| settings.cli_agent_config(&provider.id)),
    });

    let llm = match llm {
        Ok(llm) => llm,
        Err(e) => {
            return Ok(LlmConnectionReport::failure(
                &provider_id,
                &e,
                detail_for(&e),
            ))
        }
    };

    match llm.health_check().await {
        Ok(report) => Ok(LlmConnectionReport {
            ok: report.ok,
            provider_id,
            latency_ms: report.latency_ms,
            kind: None,
            detail: report.detail,
        }),
        Err(e) => Ok(LlmConnectionReport::failure(
            &provider_id,
            &e,
            detail_for(&e),
        )),
    }
}

/// Detection status of one `cli_agent/*` provider (FR-012-02): the settings
/// UI renders `detected`/`ausente` rows and the install hint from this.
#[derive(Debug, Clone, Serialize, Type)]
pub struct CliAgentStatus {
    pub provider_id: String,
    pub label: String,
    /// Bare binary name searched on PATH (`codex`, `claude`, …).
    pub binary: String,
    /// The binary resolved — to an explicit `binary_path` override or a
    /// spawnable PATHEXT match.
    pub detected: bool,
    /// Per-provider enable flag (FR-012-05). Experimental adapters can be
    /// enabled — the flag is honest about the toggle; only assistant use is
    /// gated elsewhere (`CliAgentProvider::complete` refuses non-assistant
    /// purposes for them).
    pub enabled: bool,
    /// File name of the resolved binary (`codex.cmd`, …) when detected —
    /// never the full path, which would leak the user's home dir/username.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binary_name: Option<String>,
    /// No verified non-mutating headless mode — the UI should keep this
    /// adapter listed but disabled/experimental.
    pub experimental: bool,
    /// Literal install command the UI shows when `detected` is false.
    pub install_hint: String,
}

/// Pure status computation — split out for tests. No process is spawned.
fn agent_status(spec: &cli_agent::CliAgentSpec, config: &CliAgentConfig) -> CliAgentStatus {
    let binary = cli_agent::resolve_binary(spec, config);
    CliAgentStatus {
        provider_id: spec.provider_id.to_string(),
        label: spec.label.to_string(),
        binary: spec.binary.to_string(),
        detected: binary.is_some(),
        enabled: config.enabled,
        binary_name: binary
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string()),
        experimental: spec.experimental,
        install_hint: spec.install_hint.to_string(),
    }
}

/// FR-012-02: which agent CLIs are installed. Pure PATH/PATHEXT file checks —
/// no process is spawned, so this is safe to call on every render of the
/// providers screen.
#[tauri::command]
#[specta::specta]
pub fn cli_agents_status(app: AppHandle) -> CommandResult<Vec<CliAgentStatus>> {
    let settings = settings::get_settings(&app);
    Ok(cli_agent::ADAPTERS
        .iter()
        .map(|spec| {
            let config = settings.cli_agent_config(spec.provider_id);
            agent_status(spec, &config)
        })
        .collect())
}

/// Normalize + validate a config arriving over IPC. Normalization fixes what
/// is safe to fix (blank override → PATH lookup, timeout clamp); validation
/// refuses what is not (denied extra args, invalid binary path, enabling an
/// experimental adapter). `pub(crate)`-in-module for tests.
fn normalize_cli_agent_config(
    spec: &cli_agent::CliAgentSpec,
    mut config: CliAgentConfig,
) -> Result<CliAgentConfig, CommandError> {
    config.binary_path = config
        .binary_path
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty());
    config.extra_args.retain(|arg| !arg.trim().is_empty());
    config.timeout_secs = config
        .timeout_secs
        .filter(|t| *t > 0)
        .map(|t| t.min(cli_agent::MAX_TIMEOUT_SECS));
    cli_agent::validate_config(spec, &config)
        .map_err(|e| CommandError::new(CommandErrorCode::InvalidInput, e))?;
    Ok(config)
}

/// FR-012-05: persist a `cli_agent/*` provider's config (enabled flag,
/// binary path override, extra args, timeout). Only adapter-backed provider
/// ids are accepted — HTTP providers have no cli config.
#[tauri::command]
#[specta::specta]
pub fn cli_agent_update_config(
    app: AppHandle,
    provider_id: String,
    config: CliAgentConfig,
) -> CommandResult<()> {
    let mut settings = settings::get_settings(&app);
    let provider = settings.post_process_provider(&provider_id).cloned();
    match provider {
        Some(provider) if cli_agent::is_cli_agent(&provider.id) => {
            // `is_cli_agent` implies an adapter entry, but that coupling is
            // not worth a panic in a command — degrade to a clean error.
            let spec = cli_agent::adapter_for(&provider.id).ok_or_else(|| {
                CommandError::new(
                    CommandErrorCode::Internal,
                    format!("No CLI agent adapter for provider '{provider_id}'"),
                )
            })?;
            let config = normalize_cli_agent_config(spec, config)?;
            settings.cli_agent_configs.insert(provider_id, config);
            settings::write_settings(&app, settings);
            // FR-012-17: an open panel reflects the new enabled/detected
            // state on its next snapshot (the settings fingerprint also
            // invalidates its cached provider resolution).
            crate::assistant::emit_state(&app);
            Ok(())
        }
        Some(_) => Err(CommandError::new(
            CommandErrorCode::InvalidInput,
            format!("Provider '{provider_id}' is not a CLI agent provider"),
        )),
        None => Err(CommandError::new(
            CommandErrorCode::NotFound,
            format!("Provider '{provider_id}' not found"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn summary_disabled_without_key_fr_009_21() {
        let settings = settings_with("openai", "gpt-4o-mini");
        let status = summary_status(&settings, None);
        assert!(!status.enabled);
        assert!(status.missing_api_key);
        assert!(!status.missing_model);
        assert_eq!(status.provider_id.as_deref(), Some("openai"));
        assert_eq!(status.key_hint, None);
    }

    #[test]
    fn summary_enabled_with_vaulted_key() {
        let settings = settings_with("openai", "gpt-4o-mini");
        let status = summary_status(&settings, Some("sk-proj-123456789"));
        assert!(status.enabled);
        assert!(!status.missing_api_key);
        // The hint masks the key — never the whole secret (FR-011-02).
        assert!(status.key_hint.unwrap().starts_with("••••"));
    }

    #[test]
    fn custom_endpoint_needs_no_key() {
        let settings = settings_with("custom", "llama3");
        let status = summary_status(&settings, None);
        assert!(status.enabled);
        assert!(!status.missing_api_key);
    }

    #[test]
    fn offline_mode_disables_summary() {
        let mut settings = settings_with("openai", "gpt-4o-mini");
        settings.offline_mode = true;
        let status = summary_status(&settings, Some("sk-x"));
        assert!(!status.enabled);
        assert!(status.offline);
    }

    #[test]
    fn missing_model_disables_summary() {
        let settings = settings_with("openai", "");
        let status = summary_status(&settings, Some("sk-x"));
        assert!(!status.enabled);
        assert!(status.missing_model);
    }

    #[test]
    fn cli_agent_needs_no_key_or_model() {
        // FR-012-03: no vault key required; the summary is gated on the CLI
        // being enabled and detected instead (FR-012-02).
        let settings = settings_with("cli_agent/codex", "");
        let status = summary_status(&settings, None);
        assert!(!status.missing_api_key);
        assert!(!status.missing_model);
        assert_eq!(status.provider_id.as_deref(), Some("cli_agent/codex"));
        // `enabled` itself tracks PATH detection, so it is machine-dependent;
        // the stale-override test below pins the "not detected" arm.
    }

    #[test]
    fn disabled_cli_agent_disables_summary() {
        let mut settings = settings_with("cli_agent/codex", "");
        settings.cli_agent_configs.insert(
            "cli_agent/codex".to_string(),
            CliAgentConfig {
                enabled: false,
                ..CliAgentConfig::default()
            },
        );
        let status = summary_status(&settings, None);
        assert!(!status.enabled);
        assert!(!status.missing_model);
    }

    #[test]
    fn cli_agent_with_stale_binary_override_is_unusable() {
        let mut settings = settings_with("cli_agent/codex", "");
        settings.cli_agent_configs.insert(
            "cli_agent/codex".to_string(),
            CliAgentConfig {
                binary_path: Some("C:\\no\\such\\codex.exe".to_string()),
                ..CliAgentConfig::default()
            },
        );
        let status = summary_status(&settings, None);
        assert!(!status.enabled);
    }

    #[test]
    fn experimental_cli_agents_are_never_usable_for_summaries() {
        // Even if a store claims cursor-agent is enabled and a binary is on
        // PATH, the adapter has no verified non-mutating mode: the summary
        // gate (a silent batch path) must treat it unusable. The settings
        // row still reports `experimental` and the honest `enabled` flag —
        // explicit assistant selection is the only place it can run.
        let spec = cli_agent::adapter_for("cli_agent/cursor_agent").unwrap();
        let mut settings = settings_with("cli_agent/cursor_agent", "");
        settings.cli_agent_configs.insert(
            "cli_agent/cursor_agent".to_string(),
            CliAgentConfig::default(),
        );
        let status = summary_status(&settings, None);
        assert!(!status.enabled);

        let config = CliAgentConfig::default();
        let row = agent_status(spec, &config);
        assert!(row.experimental);
        assert!(row.enabled);
    }

    #[test]
    fn agent_status_never_exposes_full_paths() {
        // The resolved binary may live under the user's home dir — only the
        // file name reaches the frontend.
        let spec = cli_agent::adapter_for("cli_agent/codex").unwrap();
        let dir = tempfile::tempdir().unwrap();
        #[cfg(windows)]
        let fake = dir.path().join("codex.exe");
        #[cfg(unix)]
        let fake = dir.path().join("codex");
        #[cfg(windows)]
        std::fs::copy(std::env::current_exe().unwrap(), &fake).unwrap();
        #[cfg(unix)]
        std::fs::write(&fake, "echo ok").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let config = CliAgentConfig {
            binary_path: Some(fake.to_string_lossy().to_string()),
            ..CliAgentConfig::default()
        };
        let row = agent_status(spec, &config);
        assert!(row.detected);
        let name = row.binary_name.unwrap();
        assert_eq!(name, fake.file_name().unwrap().to_string_lossy());
        assert!(!name.contains(['/', '\\']));
    }

    #[test]
    fn update_config_validation_rejects_denied_args_and_bad_paths() {
        let spec = cli_agent::adapter_for("cli_agent/codex").unwrap();

        // A denied flag → InvalidInput.
        let config = CliAgentConfig {
            extra_args: vec!["--sandbox".to_string(), "danger-full-access".to_string()],
            ..CliAgentConfig::default()
        };
        let err = normalize_cli_agent_config(spec, config).unwrap_err();
        assert_eq!(err.code, CommandErrorCode::InvalidInput);

        // A relative binary path → InvalidInput.
        let config = CliAgentConfig {
            binary_path: Some("codex".to_string()),
            ..CliAgentConfig::default()
        };
        assert!(normalize_cli_agent_config(spec, config).is_err());

        // Enabling an experimental adapter is allowed — the spawn-side
        // purpose gate (`complete()`) is what keeps it out of silent/batch
        // callers; the toggle itself is honest.
        let cursor = cli_agent::adapter_for("cli_agent/cursor_agent").unwrap();
        let config = CliAgentConfig {
            enabled: true,
            ..CliAgentConfig::default()
        };
        assert!(normalize_cli_agent_config(cursor, config).is_ok());
    }

    #[test]
    fn update_config_normalization_clamps_timeout_and_blanks() {
        let spec = cli_agent::adapter_for("cli_agent/codex").unwrap();
        let config = CliAgentConfig {
            binary_path: Some("   ".to_string()),
            extra_args: vec!["  ".to_string(), "--color=never".to_string()],
            timeout_secs: Some(99_999),
            ..CliAgentConfig::default()
        };
        let normalized = normalize_cli_agent_config(spec, config).unwrap();
        assert_eq!(normalized.binary_path, None);
        assert_eq!(normalized.extra_args, vec!["--color=never".to_string()]);
        assert_eq!(normalized.timeout_secs, Some(cli_agent::MAX_TIMEOUT_SECS));

        // timeout 0 keeps the "unset" semantics → None.
        let config = CliAgentConfig {
            timeout_secs: Some(0),
            ..CliAgentConfig::default()
        };
        assert_eq!(
            normalize_cli_agent_config(spec, config)
                .unwrap()
                .timeout_secs,
            None
        );
    }

    #[test]
    fn error_kinds_are_stable_and_snake_case() {
        for (error, expected) in [
            (LlmError::Auth, "auth"),
            (LlmError::Timeout, "timeout"),
            (LlmError::MissingApiKey, "missing_api_key"),
            (LlmError::Offline, "offline"),
            (LlmError::Unsupported, "unsupported"),
            (LlmError::Unavailable(503), "unavailable"),
            (LlmError::RateLimited { retry_after: None }, "rate_limited"),
            (LlmError::Provider("x".into()), "provider"),
        ] {
            let kind = LlmErrorKind::from(&error);
            assert_eq!(
                serde_json::to_value(kind).unwrap(),
                serde_json::json!(expected)
            );
        }
    }
}
