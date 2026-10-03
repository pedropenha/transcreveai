//! `cli_agent` — `LlmProvider` over locally installed agent CLIs
//! (FR-012-01..05, NFR-012-02).
//!
//! Instead of a per-token API key, these providers ride the subscription the
//! user already pays for by driving the vendor CLI headlessly (FR-012-03 —
//! authentication is the CLI's own session; nothing touches the keyring).
//!
//! Per-tool adapters (FR-012-01) declare the binary name, the non-interactive
//! argv, how the prompt is delivered, how stdout is parsed, and a cheap
//! health probe. Flags were confirmed against the versions on the dev machine
//! (codex-cli 0.159.3, Claude Code 2.1.273):
//!
//! * `codex` → `codex exec --sandbox read-only --ephemeral --skip-git-repo-check
//!   --ignore-user-config --ignore-rules --json -`. Read-only sandbox + ephemeral session: nothing is written to
//!   the repo or to `~/.codex/sessions`. `-` reads the prompt from stdin —
//!   the prompt never enters argv. On Windows npm shims are resolved to the
//!   package's native executable; batch scripts are never launched.
//!   `--json` emits JSONL events; the answer is the last `agent_message`.
//! * `claude` → `claude -p --output-format json --tools "" --strict-mcp-config
//!   --disable-slash-commands --permission-prompts none --no-session-persistence --safe-mode`.
//!   `--tools ""` disables every built-in tool (no writes, no prompts);
//!   `--strict-mcp-config` keeps MCP servers (which are *not* covered by
//!   `--tools`) from being loaded. `--safe-mode` disables hooks/plugins and
//!   customizations while preserving login. **Not** `--bare`: on 2.1.273 `--bare`
//!   refuses OAuth/keychain auth — exactly the subscription session this
//!   provider exists to reuse (FR-012-03).
//! * `cursor-agent` / `devin` → **experimental**: neither CLI has a verified
//!   non-mutating headless mode (no equivalent of codex's `--sandbox
//!   read-only` or claude's `--tools ""`). They are never auto-picked and
//!   never serve non-assistant purposes, but an *explicit* assistant
//!   provider selection may spawn them — the assistant is the interactive
//!   surface where the user opted in and watches the call, while the UI
//!   keeps flagging the adapter as experimental (NFR-012-02).
//!
//! Multi-turn requests are flattened into a single delimited transcript —
//! these CLIs are stateless per invocation (FR-012-15 only re-sends history).
//!
//! Safety (NFR-012-02): argv array without a shell, an env whitelist instead
//! of inheritance, a private empty temporary working directory — never the
//! repo), a timeout that kills the process tree, stdout capped at
//! [`MAX_STDOUT_CHARS`], stderr capped and never logged as content. The
//! prompt, argv and response body are never logged — only the provider id,
//! exit status, latency and output sizes (T-003 policy).
//!
//! User-supplied config is validated where it is persisted
//! (`cli_agent_update_config`) and re-validated at spawn time in
//! [`CliAgentProvider::complete`] (a hand-edited settings store cannot smuggle
//! argv past the denylist):
//!
//! * `extra_args` are checked against the shared deny-list plus the
//!   adapter's own additions — flags that could re-enable
//!   writes/approvals/tools or redirect config (`--sandbox`,
//!   `--dangerously-*`, `--tools`, `--mcp-config`, `-c`/`--config`,
//!   `--profile`, `--add-dir`, …) would silently defeat the hardened flags
//!   the adapters prepend. A final conservative allowlist also rejects unknown
//!   flags, positionals, short-flag clusters and option delimiters.
//! * `binary_path` must be absolute, non-UNC, canonicalizable, and its file
//!   stem must equal the adapter's binary name — otherwise one IPC call could
//!   point the provider at an arbitrary executable.
//! * `model` must match `[A-Za-z0-9._:/@-]{1,100}` — it flows into argv, and
//!   arbitrary prompt text cannot enter this settings field.
//! * `timeout_secs` is clamped to `1..=600`; `extra_args` is capped at
//!   [`MAX_EXTRA_ARGS`] entries of [`MAX_EXTRA_ARG_CHARS`] chars; NUL and
//!   newlines in args are rejected.
//!
//! Layout: [`adapters`] holds the per-tool table, [`parse`] the
//! prompt/response handling, [`validate`] the config hardening and [`spawn`]
//! binary resolution plus headless execution.

use super::provider::LlmProvider;
use super::types::{LlmError, LlmPurpose, LlmRequest, LlmResponse, LlmUsage};
use crate::settings::CliAgentConfig;
use crate::stt::types::{HealthReport, ProviderId};
use std::time::Duration;

mod adapters;
mod parse;
mod spawn;
#[cfg(test)]
mod tests;
mod validate;

pub use adapters::{adapter_for, is_cli_agent, ADAPTERS};
use parse::flatten_prompt;
pub(crate) use spawn::resolve_binary;
use spawn::{run_headless, sanitize_detail};
pub(crate) use validate::validate_config;
use validate::{is_valid_model, validate_extra_args};

/// NFR-012-03: provider output is capped so a runaway response cannot flood
/// the panel (or the meeting-summary caller). Responses at the cap are
/// truncated with a marker rather than failing.
pub const MAX_STDOUT_CHARS: usize = 32_000;

/// Bytes read from stdout before we stop accumulating — generous headroom
/// over [`MAX_STDOUT_CHARS`] for 4-byte UTF-8; the surplus is drained and
/// discarded so the child never blocks on a full pipe.
pub(crate) const MAX_STDOUT_BYTES: usize = 128 * 1024;

/// Health probes must stay cheap — they back the settings "test" button, not
/// the summary budget.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Cap on `timeout_secs` (FR-012-05): a hand-edited store must not be able to
/// pin the assistant on a CLI that never exits. `0`/`None` keeps the caller's
/// `LlmRequest::timeout`.
pub const MAX_TIMEOUT_SECS: u64 = 600;

/// Caps on `extra_args` — enough for real tuning flags, too small to be a
/// useful smuggling channel.
pub const MAX_EXTRA_ARGS: usize = 16;
pub const MAX_EXTRA_ARG_CHARS: usize = 256;

type BuildArgv = fn(model: &str, extra_args: &[String]) -> Vec<String>;
/// `truncated` = stdout hit [`MAX_STDOUT_BYTES`]: parsers must not return
/// half-parsed JSON/JSONL as plain-text completions.
type ParseOutput = fn(stdout: &[u8], truncated: bool) -> Result<(String, LlmUsage), LlmError>;
/// `Ok(exit_code, stdout)` → `Err(detail)` when the session is unusable
/// (not logged in / misconfigured). `detail` is user-facing and sanitized.
type CheckAuth = fn(&HeadlessOutput) -> Option<String>;

/// Per-tool adapter: everything `CliAgentProvider` needs to turn an
/// `LlmRequest` into a headless CLI call (FR-012-01). Extendable — add an
/// entry to [`ADAPTERS`].
#[derive(Clone)]
pub struct CliAgentSpec {
    /// `PostProcessProvider` id (`cli_agent/codex`, …).
    pub provider_id: &'static str,
    /// Binary resolved on PATH (or via `CliAgentConfig::binary_path`).
    pub binary: &'static str,
    /// Human label for the providers UI.
    pub label: &'static str,
    /// Literal install command shown when the binary is absent (FR-012-02).
    pub install_hint: &'static str,
    /// Cheap probe argv (`--version`) — proves the binary runs.
    pub version_argv: &'static [&'static str],
    /// Optional auth probe (`login status` / `auth status`) — proves the
    /// CLI's own session works when `--version` alone cannot.
    pub auth_probe_argv: Option<&'static [&'static str]>,
    /// Extra environment variable *names* the child may inherit beyond the
    /// env whitelist (e.g. `CODEX_HOME` relocates codex's config dir).
    pub extra_env: &'static [&'static str],
    /// `true` = no verified non-mutating headless mode. The adapter stays
    /// listed (the UI marks it experimental), is never auto-picked, and
    /// `complete()` refuses non-assistant purposes — an explicit assistant
    /// provider selection is the opt-in that lets it run.
    pub experimental: bool,
    /// Adapter-specific additions to the extra-args deny-list — flags whose
    /// names differ between CLIs but that would equally defeat the hardened
    /// argv this adapter prepends.
    denied_extra_args: &'static [&'static str],
    build_argv: BuildArgv,
    parse_output: ParseOutput,
    check_auth: Option<CheckAuth>,
}

/// Bounded stdout/stderr plus the exit status of a headless run.
pub(crate) struct HeadlessOutput {
    pub stdout: Vec<u8>,
    pub stdout_truncated: bool,
    pub stderr: Vec<u8>,
    pub exit_code: Option<i32>,
    pub latency_ms: u32,
}

// ---------------------------------------------------------------------------

/// `LlmProvider` driving an installed agent CLI headlessly (FR-012-01).
/// Built by `router::build_provider` when the routed provider id matches an
/// adapter; `config` carries the per-provider settings (FR-012-05).
pub struct CliAgentProvider {
    /// Backs `LlmProvider::id`.
    id: ProviderId,
    spec: &'static CliAgentSpec,
    config: CliAgentConfig,
    /// `""` = the CLI's own default model.
    model: String,
}

impl CliAgentProvider {
    pub fn new(spec: &'static CliAgentSpec, config: CliAgentConfig, model: String) -> Self {
        Self {
            id: ProviderId::from(spec.provider_id),
            spec,
            config,
            model,
        }
    }

    /// Effective timeout: the per-provider override wins (clamped to
    /// `1..=MAX_TIMEOUT_SECS` — a hand-edited store must not pin the caller);
    /// otherwise the caller's budget (60 s assistant / 180 s summary
    /// map-reduce, FR-012-05).
    fn effective_timeout(&self, req: &LlmRequest) -> Duration {
        self.config
            .timeout_secs
            .filter(|t| *t > 0)
            .map(|t| t.min(MAX_TIMEOUT_SECS))
            .map(Duration::from_secs)
            .unwrap_or(req.timeout)
    }

    fn missing_binary_error(&self) -> LlmError {
        LlmError::Provider(format!(
            "the '{}' CLI was not found on PATH",
            self.spec.binary
        ))
    }

    /// Experimental adapters have no verified non-mutating mode — refuse
    /// them for silent/batch purposes (summaries, cleanup). An *explicit*
    /// assistant provider selection is the user opt-in that lets an
    /// experimental adapter spawn (still argv-direct, no shell, kill on
    /// drop, env whitelist).
    fn experimental_error(&self) -> LlmError {
        LlmError::Provider(format!(
            "'{}' has no verified non-mutating headless mode and is disabled for this use",
            self.spec.binary
        ))
    }
}

#[async_trait::async_trait]
impl LlmProvider for CliAgentProvider {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    async fn complete(&self, req: LlmRequest) -> Result<LlmResponse, LlmError> {
        if self.spec.experimental && req.purpose != LlmPurpose::Assistant {
            return Err(self.experimental_error());
        }
        // Re-validate at spawn time — the config may have bypassed
        // `cli_agent_update_config` via a hand-edited settings store.
        validate_extra_args(self.spec, &self.config.extra_args).map_err(LlmError::Provider)?;
        if !is_valid_model(&self.model) {
            return Err(LlmError::Provider(format!(
                "model '{}' is not a valid model identifier",
                crate::utils::redact_text(&self.model)
            )));
        }
        let binary =
            resolve_binary(self.spec, &self.config).ok_or_else(|| self.missing_binary_error())?;
        let argv = (self.spec.build_argv)(&self.model, &self.config.extra_args);
        let prompt = flatten_prompt(&req);

        let out = run_headless(
            &binary,
            &argv,
            self.spec,
            Some(&prompt),
            self.effective_timeout(&req),
        )
        .await?;

        if out.stdout_truncated {
            log::warn!(
                "cli_agent {}: stdout exceeded the {} B cap — response truncated",
                self.spec.provider_id,
                MAX_STDOUT_BYTES
            );
        }
        if out.exit_code != Some(0) {
            // stderr can echo prompts or session secrets. Log only its size;
            // surfaced errors stay generic.
            log::debug!(
                "cli_agent {}: exit {:?} stderr={}B",
                self.spec.provider_id,
                out.exit_code,
                out.stderr.len()
            );
            return Err(LlmError::Provider(format!(
                "the '{}' CLI exited with code {:?}",
                self.spec.binary, out.exit_code
            )));
        }

        // A truncated stream that fails to parse is an error — never a
        // "completion" made of half-parsed JSON (the parsers themselves
        // refuse the truncated fallback path).
        let (text, usage) = (self.spec.parse_output)(&out.stdout, out.stdout_truncated)?;
        Ok(LlmResponse {
            text,
            model: if self.model.trim().is_empty() {
                self.spec.binary.to_string()
            } else {
                self.model.clone()
            },
            usage,
            provider_latency_ms: out.latency_ms,
        })
    }

    /// Cheap probe (FR-012-02): resolve the binary, run `--version`, then —
    /// where the adapter defines one — an auth probe, since a CLI that runs
    /// but is not signed in is not usable either.
    async fn health_check(&self) -> Result<HealthReport, LlmError> {
        // Probes are read-only (`--version`, auth status) — let them run for
        // experimental adapters too so the settings "test" button stays
        // useful for an explicitly selected provider.
        let binary = match resolve_binary(self.spec, &self.config) {
            Some(b) => b,
            None => {
                return Ok(HealthReport {
                    ok: false,
                    latency_ms: None,
                    detail: Some(format!(
                        "'{}' not found on PATH — {}",
                        self.spec.binary, self.spec.install_hint
                    )),
                })
            }
        };

        let version_argv: Vec<String> = self
            .spec
            .version_argv
            .iter()
            .map(|s| s.to_string())
            .collect();
        let version = run_headless(&binary, &version_argv, self.spec, None, PROBE_TIMEOUT).await?;
        if version.exit_code != Some(0) {
            return Ok(HealthReport {
                ok: false,
                latency_ms: Some(version.latency_ms as u64),
                detail: Some("the CLI failed to report its version".to_string()),
            });
        }
        let version_detail = String::from_utf8_lossy(&version.stdout)
            .lines()
            .next()
            .map(sanitize_detail)
            .filter(|l| !l.is_empty());

        if let (Some(argv), Some(check)) = (self.spec.auth_probe_argv, self.spec.check_auth) {
            let argv: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
            let probe = run_headless(&binary, &argv, self.spec, None, PROBE_TIMEOUT).await?;
            if let Some(detail) = check(&probe) {
                return Ok(HealthReport {
                    ok: false,
                    latency_ms: Some(probe.latency_ms as u64),
                    detail: Some(detail),
                });
            }
        }

        Ok(HealthReport {
            ok: true,
            latency_ms: Some(version.latency_ms as u64),
            detail: version_detail,
        })
    }
}
