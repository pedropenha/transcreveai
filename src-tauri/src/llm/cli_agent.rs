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
//! (codex-cli 0.120.0, Claude Code 2.1.273):
//!
//! * `codex` → `codex exec --sandbox read-only --ephemeral --skip-git-repo-check
//!   --json -`. Read-only sandbox + ephemeral session: nothing is written to
//!   the repo or to `~/.codex/sessions`. `-` reads the prompt from stdin —
//!   the prompt never enters argv, which matters on Windows where `.cmd`
//!   shims are wrapped by `cmd.exe` and argv is limited to ~8 KiB.
//!   `--json` emits JSONL events; the answer is the last `agent_message`.
//! * `claude` → `claude -p --output-format json --tools "" --strict-mcp-config
//!   --disable-slash-commands --permission-prompts none --no-session-persistence`.
//!   `--tools ""` disables every built-in tool (no writes, no prompts);
//!   `--strict-mcp-config` keeps MCP servers (which are *not* covered by
//!   `--tools`) from being loaded. **Not** `--bare`: on 2.1.273 `--bare`
//!   refuses OAuth/keychain auth — exactly the subscription session this
//!   provider exists to reuse (FR-012-03).
//! * `cursor-agent` / `devin` → **experimental, refused**: neither CLI has a
//!   verified non-mutating headless mode (no equivalent of codex's
//!   `--sandbox read-only` or claude's `--tools ""`). The adapters stay
//!   listed so the UI can mark them experimental/disabled, but
//!   [`CliAgentProvider::complete`] refuses to spawn them and
//!   `cli_agent_update_config` rejects `enabled: true` until a safe flag set
//!   is verified.
//!
//! Multi-turn requests are flattened into a single delimited transcript —
//! these CLIs are stateless per invocation (FR-012-15 only re-sends history).
//!
//! Safety (NFR-012-02): argv array without a shell, an env whitelist instead
//! of inheritance, a neutral working directory (the temp dir — never the
//! repo), a timeout that kills the process tree, stdout capped at
//! [`MAX_STDOUT_CHARS`], stderr redacted + capped into [`LlmError`]. The
//! prompt, argv and response body are never logged — only the provider id,
//! exit status, latency and output sizes (T-003 policy).
//!
//! User-supplied config is validated where it is persisted
//! (`cli_agent_update_config`) and re-validated at spawn time in
//! [`CliAgentProvider::complete`] (a hand-edited settings store cannot smuggle
//! argv past the denylist):
//!
//! * `extra_args` are checked against [`DENIED_EXTRA_ARGS`] + the adapter's
//!   own denylist — flags that could re-enable writes/approvals/tools or
//!   redirect config (`--sandbox`, `--dangerously-*`, `--tools`,
//!   `--mcp-config`, `-c`/`--config`, `--profile`, `--add-dir`, …) would
//!   silently defeat the hardened flags the adapters prepend.
//! * `binary_path` must be absolute, non-UNC, canonicalizable, and its file
//!   stem must equal the adapter's binary name — otherwise one IPC call could
//!   point the provider at an arbitrary executable.
//! * `model` must match `[A-Za-z0-9._:/@-]{1,100}` — it flows into argv, and
//!   `.cmd` targets go through `cmd.exe` metachar handling even with Rust's
//!   BatBadBut escaping.
//! * `timeout_secs` is clamped to `1..=600`; `extra_args` is capped at
//!   [`MAX_EXTRA_ARGS`] entries of [`MAX_EXTRA_ARG_CHARS`] chars; NUL and
//!   newlines in args are rejected.

use super::provider::LlmProvider;
use super::types::{LlmError, LlmRequest, LlmResponse, LlmUsage};
use crate::settings::CliAgentConfig;
use crate::stt::types::{HealthReport, ProviderId};
use serde_json::Value;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;

/// NFR-012-03: provider output is capped so a runaway response cannot flood
/// the panel (or the meeting-summary caller). Responses at the cap are
/// truncated with a marker rather than failing.
pub const MAX_STDOUT_CHARS: usize = 32_000;

/// Bytes read from stdout before we stop accumulating — generous headroom
/// over [`MAX_STDOUT_CHARS`] for 4-byte UTF-8; the surplus is drained and
/// discarded so the child never blocks on a full pipe.
const MAX_STDOUT_BYTES: usize = 128 * 1024;

/// stderr is diagnostic-only: summarized, redacted, and capped before it can
/// land in an `LlmError::Provider` message or a log line.
const MAX_STDERR_BYTES: usize = 8 * 1024;
const MAX_ERROR_DETAIL_CHARS: usize = 300;

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

/// After the child exits, its pipes may still be held open by a surviving
/// grandchild (`.cmd` shim → node). Give the drain tasks a short grace
/// period, then abandon them rather than hanging the whole call.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

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
    /// Extra environment variable *names* the child may inherit beyond
    /// [`ENV_WHITELIST`] (e.g. `CODEX_HOME` relocates codex's config dir).
    pub extra_env: &'static [&'static str],
    /// `true` = no verified non-mutating headless mode. The adapter stays
    /// listed (the UI can mark it experimental/disabled) but `complete()`
    /// refuses to spawn it and `cli_agent_update_config` rejects enabling it.
    pub experimental: bool,
    /// Adapter-specific additions to [`DENIED_EXTRA_ARGS`] — flags whose
    /// names differ between CLIs but that would equally defeat the hardened
    /// argv this adapter prepends.
    denied_extra_args: &'static [&'static str],
    build_argv: BuildArgv,
    parse_output: ParseOutput,
    check_auth: Option<CheckAuth>,
}

// ---------------------------------------------------------------------------
// Adapter table
// ---------------------------------------------------------------------------

fn codex_argv(model: &str, extra_args: &[String]) -> Vec<String> {
    let mut argv = vec![
        "exec".to_string(),
        "--sandbox".to_string(),
        "read-only".to_string(),
        "--ephemeral".to_string(),
        "--skip-git-repo-check".to_string(),
        "--json".to_string(),
    ];
    let model = model.trim();
    if !model.is_empty() {
        argv.push("-m".to_string());
        argv.push(model.to_string());
    }
    argv.extend(extra_args.iter().cloned());
    // `-` = read the prompt from stdin (codex exec appends piped stdin to a
    // prompt arg, so we pass *only* the marker and keep the whole prompt off
    // argv — avoids the ~8 KiB cmd.exe argv limit behind the .cmd shim).
    argv.push("-".to_string());
    argv
}

fn claude_argv(model: &str, extra_args: &[String]) -> Vec<String> {
    let mut argv = vec![
        "-p".to_string(),
        "--output-format".to_string(),
        "json".to_string(),
        "--tools".to_string(),
        String::new(),
        "--strict-mcp-config".to_string(),
        "--disable-slash-commands".to_string(),
        "--permission-prompts".to_string(),
        "none".to_string(),
        "--no-session-persistence".to_string(),
    ];
    let model = model.trim();
    if !model.is_empty() {
        argv.push("--model".to_string());
        argv.push(model.to_string());
    }
    argv.extend(extra_args.iter().cloned());
    argv
}

fn cursor_agent_argv(model: &str, extra_args: &[String]) -> Vec<String> {
    // Best-effort: cursor-agent's print mode (`-p`) accepts the prompt on
    // stdin and `--output-format text`. Not verified against a real binary.
    let mut argv = vec![
        "-p".to_string(),
        "--output-format".to_string(),
        "text".to_string(),
    ];
    let model = model.trim();
    if !model.is_empty() {
        argv.push("--model".to_string());
        argv.push(model.to_string());
    }
    argv.extend(extra_args.iter().cloned());
    argv
}

fn devin_argv(_model: &str, extra_args: &[String]) -> Vec<String> {
    // Stub — no public headless interface is documented for Devin's CLI.
    // Best guess: a print flag with the prompt on stdin. The adapter is
    // reported as absent until a real `devin` binary exists on PATH, so this
    // argv only ever runs against a user's own override.
    let mut argv = vec!["-p".to_string()];
    argv.extend(extra_args.iter().cloned());
    argv
}

/// The adapter registry. Order matters: it is also the order the providers
/// appear in the settings dropdown.
pub const ADAPTERS: &[CliAgentSpec] = &[
    CliAgentSpec {
        provider_id: "cli_agent/codex",
        binary: "codex",
        label: "Codex CLI",
        install_hint: "npm install -g @openai/codex",
        version_argv: &["--version"],
        auth_probe_argv: Some(&["login", "status"]),
        extra_env: &["CODEX_HOME"],
        experimental: false,
        // codex-specific escapes: `-s`/`--sandbox`, `-p`/`--profile`,
        // `-a`/`--ask-for-approval`, `-C`/`--cd`, `-o`/`--output-last-message`
        // (writes the answer to an arbitrary file — the read-only sandbox
        // does not cover it), `--full-auto`, `-c`/`--config` (arbitrary
        // config overrides).
        denied_extra_args: &[
            "-o",
            "--output-last-message",
            "-a",
            "--ask-for-approval",
            "--full-auto",
            "--cd",
            "--output-schema",
        ],
        build_argv: codex_argv,
        parse_output: parse_codex_jsonl,
        check_auth: Some(check_auth_exit_code),
    },
    CliAgentSpec {
        provider_id: "cli_agent/claude",
        binary: "claude",
        label: "Claude Code CLI",
        install_hint: "npm install -g @anthropic-ai/claude-code",
        version_argv: &["--version"],
        auth_probe_argv: Some(&["auth", "status"]),
        extra_env: &["CLAUDE_CONFIG_DIR"],
        experimental: false,
        // claude-specific escapes: `--settings`/`--setting-sources` reload
        // project config (which can redefine tools/permissions), `--agents`
        // and `--append-system-prompt` reshape behavior.
        denied_extra_args: &[
            "--settings",
            "--setting-sources",
            "--agents",
            "--append-system-prompt",
            "--input-format",
            "--output-format",
        ],
        build_argv: claude_argv,
        parse_output: parse_claude_json,
        check_auth: Some(check_auth_claude),
    },
    CliAgentSpec {
        provider_id: "cli_agent/cursor_agent",
        binary: "cursor-agent",
        label: "Cursor Agent CLI",
        install_hint: "cursor.com/cli",
        version_argv: &["--version"],
        auth_probe_argv: None,
        extra_env: &[],
        // No verified non-mutating mode — cursor-agent's print mode can still
        // run tools against the user's session. Refused until verified.
        experimental: true,
        denied_extra_args: &["--force", "--trust-all-tools", "--allow-all-tools"],
        build_argv: cursor_agent_argv,
        parse_output: parse_plain_text,
        check_auth: None,
    },
    CliAgentSpec {
        provider_id: "cli_agent/devin",
        binary: "devin",
        label: "Devin CLI",
        install_hint: "devin.ai",
        version_argv: &["--version"],
        auth_probe_argv: None,
        extra_env: &[],
        // No documented headless non-mutating mode — refused until verified.
        experimental: true,
        denied_extra_args: &[],
        build_argv: devin_argv,
        parse_output: parse_plain_text,
        check_auth: None,
    },
];

/// Look up the adapter behind a `PostProcessProvider` id — `None` for the
/// HTTP providers (`openai`, `anthropic`, …).
pub fn adapter_for(provider_id: &str) -> Option<&'static CliAgentSpec> {
    ADAPTERS.iter().find(|spec| spec.provider_id == provider_id)
}

pub fn is_cli_agent(provider_id: &str) -> bool {
    adapter_for(provider_id).is_some()
}

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
fn cap_text(text: String) -> String {
    if text.chars().count() <= MAX_STDOUT_CHARS {
        return text;
    }
    let mut truncated: String = text.chars().take(MAX_STDOUT_CHARS).collect();
    truncated.push_str("\n\n[response truncated]");
    truncated
}

/// Append the truncation marker to an already-trimmed, non-empty answer.
fn with_truncation_marker(mut text: String) -> String {
    text.push_str("\n\n[response truncated]");
    text
}

/// Plain-text stdout (claude/cursor/devin non-JSON paths): trim, reject
/// empty, cap. When the byte cap was hit mid-stream the marker is appended
/// even if the char cap below was not reached.
fn parse_plain_text(stdout: &[u8], truncated: bool) -> Result<(String, LlmUsage), LlmError> {
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
fn parse_codex_jsonl(stdout: &[u8], truncated: bool) -> Result<(String, LlmUsage), LlmError> {
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
fn parse_claude_json(stdout: &[u8], truncated: bool) -> Result<(String, LlmUsage), LlmError> {
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
fn check_auth_exit_code(out: &HeadlessOutput) -> Option<String> {
    if out.exit_code == Some(0) {
        None
    } else {
        Some("the CLI is not signed in or its config failed to load".to_string())
    }
}

/// `claude auth status` exits 0 even when logged out — the JSON body's
/// `loggedIn` flag is the real signal.
fn check_auth_claude(out: &HeadlessOutput) -> Option<String> {
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

// ---------------------------------------------------------------------------
// Binary detection (FR-012-02)
// ---------------------------------------------------------------------------

/// Extensions CreateProcess can run without a shell (`.cmd`/`.bat` go through
/// `cmd.exe` internally via std's batch handling). Extensionless shell
/// scripts and `.ps1` files are deliberately skipped — the first cannot be
/// spawned by CreateProcess, the second needs powershell.
#[cfg(windows)]
const SPAWNABLE_EXTENSIONS: &[&str] = &[".com", ".exe", ".bat", ".cmd"];

/// Candidate filenames for `binary` on this platform. Windows tries the
/// PATHEXT variants (filtered to spawnable extensions, honoring the user's
/// PATHEXT order); Unix wants the exact name.
#[cfg(windows)]
fn candidate_names(binary: &str, pathext: &[String]) -> Vec<String> {
    let lower = binary.to_ascii_lowercase();
    if SPAWNABLE_EXTENSIONS.iter().any(|ext| lower.ends_with(ext)) {
        return vec![binary.to_string()];
    }
    let names: Vec<String> = pathext
        .iter()
        .map(String::as_str)
        .filter(|ext| SPAWNABLE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
        .map(|ext| format!("{binary}{}", ext.to_ascii_lowercase()))
        .collect();
    // A weird/missing PATHEXT must not silently disable detection — fall
    // back to the built-in spawnable set.
    if names.is_empty() {
        return SPAWNABLE_EXTENSIONS
            .iter()
            .map(|ext| format!("{binary}{ext}"))
            .collect();
    }
    names
}

#[cfg(not(windows))]
fn candidate_names(binary: &str, _pathext: &[String]) -> Vec<String> {
    vec![binary.to_string()]
}

fn is_executable_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        return path
            .metadata()
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false);
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// First spawnable match for `binary` across `path_var`'s dirs.
fn find_on_path(binary: &str, path_var: &OsStr, pathext: &[String]) -> Option<PathBuf> {
    std::env::split_paths(path_var)
        .flat_map(|dir| {
            candidate_names(binary, pathext)
                .into_iter()
                .map(move |n| dir.join(n))
        })
        .find(|p| is_executable_file(p))
}

#[cfg(windows)]
fn pathext() -> Vec<String> {
    std::env::var("PATHEXT")
        .unwrap_or_default()
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(not(windows))]
fn pathext() -> Vec<String> {
    Vec::new()
}

/// Resolve the adapter's binary: an explicit `binary_path` override wins and
/// must survive [`validate_binary_override`] (a stale or hostile override
/// means "not detected" — better than silently falling back to another
/// install). Otherwise the binary is searched on PATH (FR-012-02). Either
/// way the canonicalized absolute path is what reaches `Command::new`.
pub(crate) fn resolve_binary(spec: &CliAgentSpec, config: &CliAgentConfig) -> Option<PathBuf> {
    if let Some(override_path) = config
        .binary_path
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        return validate_binary_override(spec, override_path).ok();
    }
    std::env::var_os("PATH")
        .and_then(|path| find_on_path(spec.binary, &path, &pathext()))
        .map(|p| p.canonicalize().unwrap_or(p))
}

// ---------------------------------------------------------------------------
// Config validation (NFR-012-02 — enforced at the update boundary *and*
// re-checked at spawn so a hand-edited store cannot bypass it)
// ---------------------------------------------------------------------------

/// Extra-arg flags that must never reach argv — each one can re-enable
/// writes/approvals/tools, redirect config, or otherwise silently defeat the
/// hardened flags the adapters prepend (extra args land *after* ours, and
/// last-wins semantics are the norm in these CLIs). Adapter-specific names
/// live in [`CliAgentSpec::denied_extra_args`].
const DENIED_EXTRA_ARGS: &[&str] = &[
    // Sandbox / approval bypass
    "--sandbox",
    "-s",
    "--dangerously-bypass-approvals-and-sandbox",
    "--dangerously-skip-permissions",
    "--approval-policy",
    // Tool / MCP control
    "--tools",
    "--allowedTools",
    "--allowed-tools",
    "--disallowedTools",
    "--disallowed-tools",
    "--mcp-config",
    "--strict-mcp-config",
    // Permission modes
    "--permission-mode",
    "--permission-prompts",
    "--permissions",
    // Config / profile / cwd redirection (`-c`/`--config` alone can redefine
    // the model, the sandbox and the tool set on codex)
    "-c",
    "--config",
    "--profile",
    "-p",
    "--add-dir",
    "-C",
    // Session state the adapters deliberately disable
    "--continue",
    "--resume",
    "--session-id",
    "--fork-session",
];

/// Whether `arg` invokes the denied `flag` — exact match, the `--flag=value`
/// form, or (for short flags) an attached value like `-cfoo`/`-c=foo`.
/// Over-matching short-flag prefixes (`-cd` hits `-c`) is intentional: a
/// hardening denylist errs on the side of refusal.
fn arg_hits_denied_flag(flag: &str, arg: &str) -> bool {
    if arg == flag {
        return true;
    }
    if flag.starts_with("--") {
        return arg.starts_with(flag) && arg[flag.len()..].starts_with('=');
    }
    // Short flag: any attached suffix counts (`-s read-only` is caught by the
    // bare match; `-sread-only`/`-s=read-only` need the prefix arm).
    arg.len() > flag.len() && arg.starts_with(flag)
}

/// Validate `extra_args` against the shared + adapter denylist and the size
/// caps ([`MAX_EXTRA_ARGS`] × [`MAX_EXTRA_ARG_CHARS`]). Also rejects NUL and
/// newline bytes — argv must stay a plain flag list. Re-checked at spawn
/// time in `complete()`.
pub(crate) fn validate_extra_args(spec: &CliAgentSpec, args: &[String]) -> Result<(), String> {
    if args.len() > MAX_EXTRA_ARGS {
        return Err(format!(
            "extra args are limited to {MAX_EXTRA_ARGS} entries"
        ));
    }
    for arg in args {
        if arg.chars().count() > MAX_EXTRA_ARG_CHARS {
            return Err(format!(
                "extra args are limited to {MAX_EXTRA_ARG_CHARS} characters each"
            ));
        }
        if arg.trim().is_empty() {
            return Err("extra args cannot contain blank entries".to_string());
        }
        if arg.contains(['\0', '\n', '\r']) {
            return Err("extra args cannot contain NUL or newline characters".to_string());
        }
        let denied = DENIED_EXTRA_ARGS
            .iter()
            .chain(spec.denied_extra_args.iter())
            .find(|flag| arg_hits_denied_flag(flag, arg));
        if let Some(flag) = denied {
            return Err(format!(
                "extra arg '{arg}' is not allowed — it conflicts with the '{flag}' safety flag"
            ));
        }
    }
    Ok(())
}

/// `model` flows into argv; `.cmd` shims go through `cmd.exe` metachar
/// handling even with Rust's BatBadBut escaping, so the accepted alphabet is
/// deliberately tight. Empty = the CLI's own default (always allowed).
pub(crate) fn is_valid_model(model: &str) -> bool {
    let model = model.trim();
    model.is_empty()
        || (model.len() <= 100
            && model
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._:/@-".contains(c)))
}

/// Validate a `binary_path` override: absolute, non-UNC, canonicalizable,
/// and the resolved file stem must equal the adapter's binary name —
/// otherwise one IPC call could point `cli_agent/*` at an arbitrary
/// executable. On Windows the extension must be spawnable (`.exe`, `.cmd`,
/// …); on unix the executable bit. Returns the canonicalized path — what
/// `Command::new` should be given.
pub(crate) fn validate_binary_override(spec: &CliAgentSpec, path: &str) -> Result<PathBuf, String> {
    if path.contains('\0') {
        return Err("binary path contains a NUL byte".to_string());
    }
    let path = Path::new(path);
    if !path.is_absolute() {
        return Err("binary path must be absolute".to_string());
    }
    #[cfg(windows)]
    {
        // UNC (`\\server\share`, `\\?\UNC\…`) can resolve to a remote binary —
        // refuse it outright. A verbatim `\\?\C:\…` prefix is fine: it is
        // what `canonicalize` returns anyway, and the stem check below still
        // applies to it.
        use std::path::{Component, Prefix};
        if let Some(Component::Prefix(prefix)) = path.components().next() {
            match prefix.kind() {
                Prefix::UNC(..) | Prefix::VerbatimUNC(..) => {
                    return Err("UNC binary paths are not allowed".to_string())
                }
                _ => {}
            }
        }
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| "binary path does not resolve to an existing file".to_string())?;
    if !is_executable_file(&canonical) {
        return Err("binary path is not an executable file".to_string());
    }
    let stem = canonical
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    if !stem.eq_ignore_ascii_case(spec.binary) {
        return Err(format!(
            "binary path must point to a '{}' executable",
            spec.binary
        ));
    }
    #[cfg(windows)]
    {
        let ext = canonical
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if !SPAWNABLE_EXTENSIONS.contains(&format!(".{ext}").as_str()) {
            return Err(format!(
                "binary path must end in a spawnable extension ({ext})"
            ));
        }
    }
    Ok(canonical)
}

/// Whole-config check used by `cli_agent_update_config`. Normalization
/// (blank override → `None`, timeout clamp) happens at the command layer;
/// this refuses what normalization cannot fix.
pub(crate) fn validate_config(spec: &CliAgentSpec, config: &CliAgentConfig) -> Result<(), String> {
    if spec.experimental && config.enabled {
        return Err(format!(
            "'{}' has no verified non-mutating headless mode and stays disabled",
            spec.binary
        ));
    }
    if let Some(path) = config.binary_path.as_deref() {
        validate_binary_override(spec, path)?;
    }
    validate_extra_args(spec, &config.extra_args)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Sanitized child environment (NFR-012-02)
// ---------------------------------------------------------------------------

/// The only variables a CLI child inherits. Covers what the CLIs actually
/// need — PATH to resolve their own helpers, HOME/USERPROFILE + the XDG and
/// config dirs where the CLI session lives (auth is the CLI's own login
/// state, FR-012-03), temp dirs, locale, proxies for corporate networks.
/// Everything else — including any `*_API_KEY` the user's shell happens to
/// export — stays out.
///
/// `COMSPEC` is deliberately absent: a user-supplied `cmd.exe` shim path is
/// not something the child should inherit (system binaries we spawn
/// ourselves resolve via `%SystemRoot%\System32`, see [`system_tool`]).
/// Note `HTTP_PROXY`/`HTTPS_PROXY` may embed credentials
/// (`http://user:pass@proxy`) — they are forwarded because the CLI cannot
/// reach its backend on corporate networks without them, and the child is
/// the user's own logged-in agent anyway.
const ENV_WHITELIST: &[&str] = &[
    "PATH",
    "PATHEXT",
    "SYSTEMROOT",
    "WINDIR",
    "TEMP",
    "TMP",
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    "APPDATA",
    "LOCALAPPDATA",
    "PROGRAMDATA",
    "PROGRAMFILES",
    "USERNAME",
    "HOME",
    "USER",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TERM",
    "COLORTERM",
    "NO_COLOR",
    "XDG_CONFIG_HOME",
    "XDG_CACHE_HOME",
    "XDG_DATA_HOME",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
];

/// Secret-shaped env names are never forwarded to the child — not even via
/// an adapter's `extra_env`. Auth is the CLI's own session (FR-012-03); a
/// leaked `OPENAI_API_KEY` would silently re-key it (and would be readable
/// by anything the agent runs).
fn is_secret_env_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    [
        "_API_KEY",
        "_API_TOKEN",
        "_ACCESS_TOKEN",
        "_AUTH_TOKEN",
        "_SECRET",
        "_SECRET_KEY",
        "_PASSWORD",
        "_PRIVATE_KEY",
    ]
    .iter()
    .any(|suffix| upper.ends_with(suffix))
        // Common bare names the suffixes above do not catch.
        || matches!(
            upper.as_str(),
            "GH_TOKEN" | "GITHUB_TOKEN" | "GITLAB_TOKEN" | "AWS_SESSION_TOKEN"
        )
}

fn sanitized_env(extra: &[&str]) -> Vec<(OsString, OsString)> {
    ENV_WHITELIST
        .iter()
        .chain(extra.iter())
        .filter(|name| !is_secret_env_name(name))
        .filter_map(|name| std::env::var_os(name).map(|v| (OsString::from(name), v)))
        .collect()
}

// ---------------------------------------------------------------------------
// Headless spawn (NFR-012-02)
// ---------------------------------------------------------------------------

pub(crate) struct HeadlessOutput {
    pub stdout: Vec<u8>,
    pub stdout_truncated: bool,
    pub stderr: Vec<u8>,
    pub exit_code: Option<i32>,
    pub latency_ms: u32,
}

/// Read a pipe to EOF, keeping at most `cap` bytes but draining the rest so
/// the child never deadlocks on a full buffer. Returns (kept, truncated).
async fn read_capped(mut reader: impl tokio::io::AsyncRead + Unpin, cap: usize) -> (Vec<u8>, bool) {
    let mut kept = Vec::new();
    let mut truncated = false;
    let mut buf = [0u8; 8192];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let room = cap.saturating_sub(kept.len());
                kept.extend_from_slice(&buf[..n.min(room)]);
                if n > room {
                    truncated = true;
                }
            }
        }
    }
    (kept, truncated)
}

/// Token shapes `utils::redact_secret_patterns` does not cover but agent CLI
/// stderr realistically carries: GitHub PATs (`ghp_`, `gho_`, `github_pat_`),
/// Anthropic keys (`sk-ant-`), Google API keys (`AIza`), Slack tokens
/// (`xox…`), and JWTs (`eyJ…` — base64url header prefix).
fn redact_cli_secret_patterns(text: &str) -> std::borrow::Cow<'_, str> {
    use regex::Regex;
    use std::sync::LazyLock;

    static PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
        [
            r"\bgh[pousr]_[A-Za-z0-9]{16,}",
            r"\bgithub_pat_[A-Za-z0-9_]{16,}",
            r"\bsk-ant-[A-Za-z0-9_-]{4,}",
            r"\bAIza[A-Za-z0-9_-]{10,}",
            r"\bxox[baprs]-[A-Za-z0-9-]{6,}",
            // JWT: three base64url segments, first starting with eyJ
            r"\beyJ[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]{5,}",
        ]
        .iter()
        .map(|p| Regex::new(p).expect("static secret redaction pattern must compile"))
        .collect()
    });

    let mut redacted: std::borrow::Cow<'_, str> = text.into();
    for pattern in PATTERNS.iter() {
        if pattern.is_match(&redacted) {
            redacted = pattern
                .replace_all(&redacted, "[REDACTED]")
                .into_owned()
                .into();
        }
    }
    redacted
}

/// Scrub the user's home dir and `Users\<name>`/`/home/<name>` path segments
/// so a machine/username never leaks into an error the UI can show.
fn strip_user_paths(text: &str) -> String {
    let mut out = text.to_string();
    for var in ["USERPROFILE", "HOME"] {
        if let Ok(home) = std::env::var(var) {
            if !home.is_empty() {
                out = out.replace(&home, "~");
                // Windows tools often emit forward-slash paths.
                out = out.replace(&home.replace('\\', "/"), "~");
            }
        }
    }
    use regex::Regex;
    use std::sync::LazyLock;
    static USERS_DIR: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)([A-Za-z]:[/\\]+Users[/\\]+|/Users/|/home/)[^/\\\s]+")
            .expect("users-dir pattern must compile")
    });
    USERS_DIR.replace_all(&out, "${1}<user>").into_owned()
}

/// Trim + redact + cap a diagnostic blob before it can reach an error
/// message or a log line (T-003 policy). Secret redaction covers both the
/// shared key formats and CLI-specific ones; home/username paths are
/// stripped so errors do not identify the machine's user.
fn sanitize_detail(raw: &str) -> String {
    let redacted = crate::utils::redact_secret_patterns(raw.trim());
    let redacted = redact_cli_secret_patterns(&redacted);
    let redacted = strip_user_paths(&redacted);
    redacted
        .chars()
        .take(MAX_ERROR_DETAIL_CHARS)
        .collect::<String>()
}

/// Resolve a Windows system binary from `%SystemRoot%\System32` — never via
/// PATH, which can be attacker-influenced through the environment.
#[cfg(windows)]
fn system_tool(name: &str) -> PathBuf {
    let root = std::env::var_os("SystemRoot")
        .or_else(|| std::env::var_os("WINDIR"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    root.join("System32").join(name)
}

/// Kill the spawned process **tree**. `.cmd`/`.bat` shims go through
/// `cmd.exe`, so the real CLI is a grandchild (`codex.cmd` → `cmd.exe` →
/// `node.exe`); a bare `kill()` would orphan it mid-call and leave it holding
/// the captured pipes. `taskkill /T` walks the tree. On unix the child is
/// spawned in its own process group (`process_group(0)`), so `kill(-pgid)`
/// takes the grandchildren with it.
async fn kill_process_tree(child: &mut tokio::process::Child) {
    #[cfg(windows)]
    if let Some(pid) = child.id() {
        let _ = tokio::process::Command::new(system_tool("taskkill.exe"))
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
    }
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        // pgid == pid (process_group(0) at spawn). Negative pid = whole
        // group; ESRCH just means it already exited.
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    let _ = child.kill().await;
}

/// Spawn `binary argv…` with a sanitized environment, an optional stdin
/// payload, and a hard timeout that kills the child (NFR-012-02). The
/// working directory is the temp dir — never the repo — so even a
/// misconfigured sandbox cannot touch the project's files, and codex's
/// read-only sandbox scopes reads to a throwaway tree.
async fn run_headless(
    binary: &Path,
    argv: &[String],
    spec: &CliAgentSpec,
    stdin_payload: Option<&str>,
    timeout: Duration,
) -> Result<HeadlessOutput, LlmError> {
    let mut cmd = tokio::process::Command::new(binary);
    cmd.args(argv)
        .env_clear()
        .envs(sanitized_env(spec.extra_env))
        .current_dir(std::env::temp_dir())
        .stdin(if stdin_payload.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Belt-and-suspenders cancel: dropping the future mid-flight kills
        // the child too (FR-012 / AC-012-05 groundwork).
        .kill_on_drop(true);

    #[cfg(unix)]
    {
        // Own process group → `kill_process_tree` can signal grandchildren.
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        // A headless helper must never pop a console window (these calls run
        // while the user is dictating).
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = cmd.spawn().map_err(|e| {
        log::warn!(
            "cli_agent {}: failed to spawn '{}': {e}",
            spec.provider_id,
            spec.binary
        );
        LlmError::Provider(format!("could not start the '{}' CLI", spec.binary))
    })?;

    // Feed the prompt on stdin from a task so a slow reader cannot block
    // the wait below; a broken pipe just means the child exited early. The
    // handle is kept so a timeout can detach the writer instead of leaking
    // a task holding the dead child's pipe.
    let stdin_task = if let (Some(mut stdin), Some(payload)) = (child.stdin.take(), stdin_payload) {
        let payload = payload.to_string();
        Some(tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            let _ = stdin.write_all(payload.as_bytes()).await;
            let _ = stdin.shutdown().await;
        }))
    } else {
        None
    };

    let stdout_task = tokio::spawn({
        let stdout = child.stdout.take();
        async move {
            match stdout {
                Some(pipe) => read_capped(pipe, MAX_STDOUT_BYTES).await,
                None => (Vec::new(), false),
            }
        }
    });
    let stderr_task = tokio::spawn({
        let stderr = child.stderr.take();
        async move {
            match stderr {
                Some(pipe) => read_capped(pipe, MAX_STDERR_BYTES).await,
                None => (Vec::new(), false),
            }
        }
    });

    let started = Instant::now();
    let status = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(status) => status.map_err(|e| {
            log::warn!("cli_agent {}: wait failed: {e}", spec.provider_id);
            LlmError::Provider(format!("the '{}' CLI failed", spec.binary))
        })?,
        Err(_) => {
            log::warn!(
                "cli_agent {}: killed after {} s timeout",
                spec.provider_id,
                timeout.as_secs()
            );
            if let Some(task) = stdin_task {
                task.abort();
            }
            kill_process_tree(&mut child).await;
            // Drain tasks may still be blocked on pipes a surviving
            // grandchild inherited — the result is a timeout either way, so
            // the captured prefix is abandoned rather than awaited.
            stdout_task.abort();
            stderr_task.abort();
            return Err(LlmError::Timeout);
        }
    };
    let latency_ms = started.elapsed().as_millis() as u32;

    // The child exited, but a grandchild may still hold our pipes open —
    // give the drains a short grace period, then abort them rather than
    // hanging the call (or detaching a task holding a dead pipe).
    let mut stdout_task = stdout_task;
    let mut stderr_task = stderr_task;
    let (stdout, stdout_truncated) =
        match tokio::time::timeout(DRAIN_TIMEOUT, &mut stdout_task).await {
            Ok(joined) => joined.unwrap_or_default(),
            Err(_) => {
                stdout_task.abort();
                log::warn!(
                    "cli_agent {}: stdout pipe held open after exit — draining abandoned",
                    spec.provider_id
                );
                (Vec::new(), true)
            }
        };
    let (stderr, _) = match tokio::time::timeout(DRAIN_TIMEOUT, &mut stderr_task).await {
        Ok(joined) => joined.unwrap_or_default(),
        Err(_) => {
            stderr_task.abort();
            log::warn!(
                "cli_agent {}: stderr pipe held open after exit — draining abandoned",
                spec.provider_id
            );
            (Vec::new(), false)
        }
    };
    if let Some(task) = stdin_task {
        task.abort();
    }

    log::debug!(
        "cli_agent {}: exit={:?} latency={}ms stdout={}B truncated={}",
        spec.provider_id,
        status.code(),
        latency_ms,
        stdout.len(),
        stdout_truncated,
    );

    Ok(HeadlessOutput {
        stdout,
        stdout_truncated,
        stderr,
        exit_code: status.code(),
        latency_ms,
    })
}

// ---------------------------------------------------------------------------
// The provider
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
    /// rather than trusting a prompt-flattened dictation transcript to a CLI
    /// that can edit files or run commands under the user's session.
    fn experimental_error(&self) -> LlmError {
        LlmError::Provider(format!(
            "'{}' has no verified non-mutating headless mode and is disabled",
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
        if self.spec.experimental {
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
            // stderr can carry the CLI's session paths and stray tokens — it
            // is sanitized, then kept behind debug-mode gating; the surfaced
            // error stays generic.
            let detail = sanitize_detail(&String::from_utf8_lossy(&out.stderr));
            log::debug!(
                "cli_agent {}: exit {:?} stderr: {}",
                self.spec.provider_id,
                out.exit_code,
                crate::utils::redact_text(&detail)
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
        if self.spec.experimental {
            return Ok(HealthReport {
                ok: false,
                latency_ms: None,
                detail: Some(
                    "experimental provider — disabled until a non-mutating mode is verified"
                        .to_string(),
                ),
            });
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::types::{LlmMessage, LlmPurpose, LlmRole};

    fn req(system: &str, user: &str) -> LlmRequest {
        LlmRequest {
            system: system.to_string(),
            messages: vec![LlmMessage::user(user)],
            max_tokens: 100,
            temperature: 0.0,
            timeout: Duration::from_secs(30),
            purpose: LlmPurpose::Summary,
        }
    }

    fn config() -> CliAgentConfig {
        CliAgentConfig::default()
    }

    // -- prompt flattening ---------------------------------------------------

    #[test]
    fn flatten_prompt_wraps_roles_in_tags() {
        let mut r = req("Be terse.", "Summarize this.");
        r.messages.push(LlmMessage {
            role: LlmRole::Assistant,
            content: "Sure.".to_string(),
        });
        r.messages.push(LlmMessage::user("And the title?"));

        let flat = flatten_prompt(&r);
        assert!(flat.starts_with("<system>\nBe terse.\n</system>"));
        assert!(flat.contains("<user>\nSummarize this.\n</user>"));
        assert!(flat.contains("<assistant>\nSure.\n</assistant>"));
        assert!(flat.ends_with("<user>\nAnd the title?\n</user>"));
    }

    #[test]
    fn flatten_prompt_omits_empty_system() {
        let flat = flatten_prompt(&req("   ", "hi"));
        assert!(!flat.contains("<system>"));
        assert_eq!(flat, "<user>\nhi\n</user>");
    }

    // -- argv construction ---------------------------------------------------

    #[test]
    fn codex_argv_is_read_only_ephemeral_and_stdin_prompted() {
        let argv = codex_argv("", &[]);
        assert_eq!(argv[0], "exec");
        assert!(argv
            .windows(2)
            .any(|w| w[0] == "--sandbox" && w[1] == "read-only"));
        assert!(argv.contains(&"--ephemeral".to_string()));
        assert!(argv.contains(&"--json".to_string()));
        // `-` must be last so extra args cannot move the prompt marker.
        assert_eq!(argv.last().unwrap(), "-");
        assert!(!argv.iter().any(|a| a.contains("--model")));
    }

    #[test]
    fn codex_argv_places_model_and_extra_args_before_stdin_marker() {
        let extra = vec!["--foo".to_string(), "bar".to_string()];
        let argv = codex_argv("gpt-5.1", &extra);
        let pos_model = argv.iter().position(|a| a == "-m").unwrap();
        assert_eq!(argv[pos_model + 1], "gpt-5.1");
        assert_eq!(argv.last().unwrap(), "-");
        let pos_extra = argv.iter().position(|a| a == "--foo").unwrap();
        assert!(pos_extra < argv.len() - 1);
    }

    #[test]
    fn claude_argv_disables_all_tools_and_session_persistence() {
        let argv = claude_argv("", &[]);
        assert_eq!(argv[0], "-p");
        // `--tools ""` — the empty-string arg disables every built-in tool.
        let pos = argv.iter().position(|a| a == "--tools").unwrap();
        assert_eq!(argv[pos + 1], "");
        assert!(argv.contains(&"--strict-mcp-config".to_string()));
        assert!(argv.contains(&"--no-session-persistence".to_string()));
        assert!(argv
            .windows(2)
            .any(|w| w[0] == "--output-format" && w[1] == "json"));
        // `--bare` is banned: it refuses OAuth/keychain auth (FR-012-03).
        assert!(!argv.contains(&"--bare".to_string()));
    }

    // -- argv pins (every adapter's hardened argv is asserted verbatim) ------

    #[test]
    fn adapter_argvs_are_pinned() {
        assert_eq!(
            codex_argv("", &[]),
            vec![
                "exec",
                "--sandbox",
                "read-only",
                "--ephemeral",
                "--skip-git-repo-check",
                "--json",
                "-",
            ]
        );
        assert_eq!(
            claude_argv("", &[]),
            vec![
                "-p",
                "--output-format",
                "json",
                "--tools",
                "",
                "--strict-mcp-config",
                "--disable-slash-commands",
                "--permission-prompts",
                "none",
                "--no-session-persistence",
            ]
        );
        assert_eq!(
            cursor_agent_argv("", &[]),
            vec!["-p", "--output-format", "text"]
        );
        assert_eq!(devin_argv("", &[]), vec!["-p"]);
        assert_eq!(
            claude_argv("claude-sonnet-4", &[]),
            vec![
                "-p",
                "--output-format",
                "json",
                "--tools",
                "",
                "--strict-mcp-config",
                "--disable-slash-commands",
                "--permission-prompts",
                "none",
                "--no-session-persistence",
                "--model",
                "claude-sonnet-4",
            ]
        );
    }

    // -- extra_args validation ------------------------------------------------

    fn spec(id: &str) -> &'static CliAgentSpec {
        adapter_for(id).unwrap()
    }

    #[test]
    fn extra_args_reject_hardening_defeat_flags() {
        for denied in [
            "--sandbox",
            "--sandbox=read-only",
            "--dangerously-bypass-approvals-and-sandbox",
            "--dangerously-skip-permissions",
            "--tools",
            "--tools=Bash",
            "--allowedTools",
            "--allowedTools=Bash",
            "--permission-mode",
            "--permission-mode=bypassPermissions",
            "--mcp-config",
            "--mcp-config=/tmp/evil.json",
            "--add-dir",
            "--add-dir=/repo",
            "-c",
            "-cfoo=bar",
            "--config",
            "--profile",
            "--profile=default",
            "--continue",
            "--resume",
        ] {
            for s in ["cli_agent/codex", "cli_agent/claude"].map(spec) {
                assert!(
                    validate_extra_args(s, &[denied.to_string()]).is_err(),
                    "{denied} must be rejected for {}",
                    s.provider_id
                );
            }
        }
    }

    #[test]
    fn extra_args_reject_adapter_specific_flags() {
        // codex-only escapes.
        for denied in [
            "-o",
            "--output-last-message",
            "--full-auto",
            "--cd",
            "-C",
            "-a",
        ] {
            assert!(
                validate_extra_args(spec("cli_agent/codex"), &[denied.to_string()]).is_err(),
                "{denied} must be rejected for codex"
            );
        }
        // claude-only escapes.
        for denied in ["--settings", "--setting-sources", "--agents"] {
            assert!(
                validate_extra_args(spec("cli_agent/claude"), &[denied.to_string()]).is_err(),
                "{denied} must be rejected for claude"
            );
        }
    }

    #[test]
    fn extra_args_accept_benign_flags() {
        let args = vec![
            "--search".to_string(),
            "--verbose".to_string(),
            "value with spaces".to_string(),
        ];
        assert!(validate_extra_args(spec("cli_agent/codex"), &args).is_ok());
        assert!(validate_extra_args(spec("cli_agent/claude"), &args).is_ok());
    }

    #[test]
    fn extra_args_respect_caps_and_reject_control_chars() {
        let spec = spec("cli_agent/codex");
        let too_many = vec!["--x".to_string(); MAX_EXTRA_ARGS + 1];
        assert!(validate_extra_args(spec, &too_many).is_err());
        let too_long = vec!["a".repeat(MAX_EXTRA_ARG_CHARS + 1)];
        assert!(validate_extra_args(spec, &too_long).is_err());
        for bad in ["--x\0y", "--x\ny", "--x\ry", "   "] {
            assert!(
                validate_extra_args(spec, &[bad.to_string()]).is_err(),
                "{bad:?} must be rejected"
            );
        }
        let max_ok = vec!["a".repeat(MAX_EXTRA_ARG_CHARS); MAX_EXTRA_ARGS];
        assert!(validate_extra_args(spec, &max_ok).is_ok());
    }

    // -- model validation -----------------------------------------------------

    #[test]
    fn model_validation_accepts_only_a_tight_alphabet() {
        for ok in [
            "",
            "   ",
            "gpt-5.1",
            "claude-sonnet-4-20250514",
            "accounts/fireworks/models/llama-v3p1",
            "hf.co/org/model:latest",
            "gpt-5.1@2026-01-01",
        ] {
            assert!(is_valid_model(ok), "{ok:?} should be valid");
        }
        let overlong = "m".repeat(101);
        for bad in [
            "model;rm -rf /",
            "model&calc",
            "model|whoami",
            "model`id`",
            "model$(id)",
            "model\n--sandbox",
            "model name",
            "çodex",
            overlong.as_str(),
        ] {
            assert!(!is_valid_model(bad), "{bad:?} should be invalid");
        }
    }

    // -- output parsing ------------------------------------------------------

    #[test]
    fn codex_jsonl_extracts_agent_messages_and_usage() {
        let stdout = concat!(
            r#"{"type":"thread.started","thread_id":"t1"}"#,
            "\n",
            r#"{"type":"item.completed","item":{"type":"reasoning","text":"thinking"}}"#,
            "\n",
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"The answer."}}"#,
            "\n",
            r#"{"type":"turn.completed","usage":{"input_tokens":10,"output_tokens":4}}"#,
        );
        let (text, usage) = parse_codex_jsonl(stdout.as_bytes(), false).unwrap();
        assert_eq!(text, "The answer.");
        assert_eq!(usage.input_tokens, Some(10));
        assert_eq!(usage.output_tokens, Some(4));
    }

    #[test]
    fn codex_jsonl_turn_failed_becomes_provider_error() {
        let stdout = r#"{"type":"turn.failed","error":{"message":"model exploded"}}"#;
        match parse_codex_jsonl(stdout.as_bytes(), false) {
            Err(LlmError::Provider(msg)) => assert!(msg.contains("model exploded")),
            other => panic!("expected Provider error, got {other:?}"),
        }
    }

    #[test]
    fn codex_jsonl_falls_back_to_plain_text() {
        let (text, _) = parse_codex_jsonl(b"  plain answer  ", false).unwrap();
        assert_eq!(text, "plain answer");
    }

    #[test]
    fn claude_json_reads_result_and_usage() {
        let stdout = r#"{"type":"result","is_error":false,"result":"hi there","usage":{"input_tokens":3,"output_tokens":2}}"#;
        let (text, usage) = parse_claude_json(stdout.as_bytes(), false).unwrap();
        assert_eq!(text, "hi there");
        assert_eq!(usage.input_tokens, Some(3));
    }

    #[test]
    fn claude_json_is_error_maps_to_provider_error() {
        let stdout = r#"{"type":"result","is_error":true,"result":"hit a limit"}"#;
        match parse_claude_json(stdout.as_bytes(), false) {
            Err(LlmError::Provider(msg)) => assert!(msg.contains("hit a limit")),
            other => panic!("expected Provider error, got {other:?}"),
        }
    }

    #[test]
    fn plain_text_rejects_empty_output() {
        assert!(matches!(
            parse_plain_text(b"   \n ", false),
            Err(LlmError::Provider(_))
        ));
    }

    #[test]
    fn truncated_json_never_returns_partially_parsed_text() {
        // A claude JSON result cut by the byte cap mid-string must error —
        // the fallback to "plain text" would otherwise surface half-parsed
        // JSON as the completion.
        let cut = br#"{"type":"result","is_error":false,"result":"hi th"#;
        assert!(matches!(
            parse_claude_json(cut, true),
            Err(LlmError::Provider(_))
        ));
        // Same for codex JSONL with zero parseable events.
        assert!(matches!(
            parse_codex_jsonl(cut, true),
            Err(LlmError::Provider(_))
        ));
        // Untruncated: the same bytes take the plain-text fallback.
        assert!(parse_claude_json(cut, false).is_ok());
        // Truncated JSONL with a complete agent message keeps the parsed
        // part and marks the cut.
        let stdout = concat!(
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"The answer."}}"#,
            "\n",
            r#"{"type":"item.completed","item":{"type":"agent_mes"#,
        );
        let (text, _) = parse_codex_jsonl(stdout.as_bytes(), true).unwrap();
        assert_eq!(text, "The answer.\n\n[response truncated]");
        // Truncated plain text is still returned, marked.
        let (text, _) = parse_plain_text(b"a plain answer", true).unwrap();
        assert_eq!(text, "a plain answer\n\n[response truncated]");
    }

    #[test]
    fn cap_text_truncates_with_marker() {
        let long = "x".repeat(MAX_STDOUT_CHARS + 100);
        let capped = cap_text(long);
        assert!(capped.ends_with("[response truncated]"));
        assert!(capped.chars().count() <= MAX_STDOUT_CHARS + 30);
    }

    // -- detection -----------------------------------------------------------

    #[cfg(windows)]
    #[test]
    fn find_on_path_prefers_spawnable_extensions() {
        let dir = tempfile::tempdir().unwrap();
        // An extensionless shim must not win over a spawnable .cmd — node
        // installs CLIs exactly like this (`codex` + `codex.cmd`).
        std::fs::write(dir.path().join("faketool"), "shell script").unwrap();
        std::fs::write(dir.path().join("faketool.cmd"), "@echo hi").unwrap();
        let path = OsString::from(dir.path());
        let found = find_on_path(
            "faketool",
            &path,
            &[".COM".into(), ".EXE".into(), ".CMD".into()],
        );
        assert_eq!(found.unwrap().file_name().unwrap(), "faketool.cmd");
        assert!(find_on_path("nothere", &path, &[".EXE".into()]).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn find_on_path_requires_the_executable_bit() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("faketool");
        std::fs::write(&plain, "#!/bin/sh\n").unwrap();
        let path = OsString::from(dir.path());
        assert!(find_on_path("faketool", &path, &[]).is_none());
        std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(find_on_path("faketool", &path, &[]).is_some());
    }

    #[test]
    fn binary_override_must_exist() {
        let spec = adapter_for("cli_agent/codex").unwrap();
        let mut cfg = config();
        cfg.binary_path = Some("C:\\definitely\\not\\a\\real\\codex.exe".to_string());
        assert!(resolve_binary(spec, &cfg).is_none());
    }

    #[test]
    fn binary_override_rejects_relative_unc_and_wrong_stem() {
        let spec = adapter_for("cli_agent/codex").unwrap();
        let dir = tempfile::tempdir().unwrap();
        let codex = echo_tool_named(dir.path(), "codex");

        // A correctly-named existing file resolves to its canonical path.
        let resolved = validate_binary_override(spec, &codex.to_string_lossy()).unwrap();
        assert!(resolved.is_absolute());
        assert_eq!(
            resolved.file_name().unwrap().to_string_lossy(),
            codex.file_name().unwrap().to_string_lossy()
        );

        // Relative paths — even to a real file — are refused.
        assert!(validate_binary_override(spec, "codex").is_err());
        #[cfg(windows)]
        assert!(validate_binary_override(spec, r".\codex.cmd").is_err());
        #[cfg(unix)]
        assert!(validate_binary_override(spec, "./codex").is_err());

        // A file whose stem does not match the adapter's binary name.
        let impostor = echo_tool_named(dir.path(), "notcodex");
        assert!(validate_binary_override(spec, &impostor.to_string_lossy()).is_err());

        // A nonexistent absolute path.
        #[cfg(windows)]
        let ghost = r"C:\no\such\dir\codex.exe";
        #[cfg(unix)]
        let ghost = "/no/such/dir/codex";
        assert!(validate_binary_override(spec, ghost).is_err());

        // `resolve_binary` maps every rejection to "not detected".
        let mut cfg = config();
        cfg.binary_path = Some(impostor.to_string_lossy().to_string());
        assert!(resolve_binary(spec, &cfg).is_none());
    }

    #[cfg(windows)]
    #[test]
    fn binary_override_rejects_unc_paths() {
        let spec = adapter_for("cli_agent/codex").unwrap();
        for unc in [
            r"\\server\share\codex.exe",
            r"\\?\UNC\server\share\codex.exe",
        ] {
            assert!(
                validate_binary_override(spec, unc).is_err(),
                "{unc} must be rejected"
            );
        }
        // A verbatim `\\?\C:\…` path is allowed only if it canonicalizes to
        // the right stem.
        let dir = tempfile::tempdir().unwrap();
        let codex = echo_tool_named(dir.path(), "codex");
        let plain = codex.to_string_lossy().to_string();
        let verbatim_input = format!(r"\\?\{plain}");
        let resolved = validate_binary_override(spec, &verbatim_input).unwrap();
        assert!(resolved.file_stem().unwrap() == "codex");
    }

    #[test]
    fn spawn_time_extra_args_denial_is_per_adapter() {
        // The shared list applies to every adapter…
        assert!(validate_extra_args(
            adapter_for("cli_agent/devin").unwrap(),
            &["--sandbox".to_string()]
        )
        .is_err());
    }

    // -- env sanitization ----------------------------------------------------

    #[test]
    fn sanitized_env_keeps_whitelist_and_adapter_extras_only() {
        std::env::set_var("CLI_AGENT_TEST_SECRET", "s3cret");
        let env = sanitized_env(&["CLI_AGENT_TEST_EXTRA"]);
        let names: Vec<String> = env
            .iter()
            .map(|(k, _)| k.to_string_lossy().to_string())
            .collect();
        assert!(names.iter().any(|n| n == "PATH"));
        assert!(!names.iter().any(|n| n == "CLI_AGENT_TEST_SECRET"));
        assert!(!names.iter().any(|n| n == "CLI_AGENT_TEST_EXTRA")); // unset → absent
        std::env::set_var("CLI_AGENT_TEST_EXTRA", "x");
        let names: Vec<String> = sanitized_env(&["CLI_AGENT_TEST_EXTRA"])
            .iter()
            .map(|(k, _)| k.to_string_lossy().to_string())
            .collect();
        assert!(names.iter().any(|n| n == "CLI_AGENT_TEST_EXTRA"));
        std::env::remove_var("CLI_AGENT_TEST_SECRET");
        std::env::remove_var("CLI_AGENT_TEST_EXTRA");
    }

    #[test]
    fn sanitized_env_never_forwards_api_keys_or_comspec() {
        // Keys the user's shell exports must not leak into the CLI child
        // (auth is the CLI's own session, FR-012-03 — a *_API_KEY in env
        // could silently re-key it).
        std::env::set_var("OPENAI_API_KEY", "sk-test");
        std::env::set_var("ANTHROPIC_API_KEY", "sk-ant-test");
        std::env::set_var("COMSPEC", "C:\\evil\\cmd.exe");
        let names: Vec<String> = sanitized_env(&["OPENAI_API_KEY"])
            .iter()
            .map(|(k, _)| k.to_string_lossy().to_string())
            .collect();
        for banned in ["OPENAI_API_KEY", "ANTHROPIC_API_KEY", "COMSPEC"] {
            assert!(!names.iter().any(|n| n == banned), "{banned} leaked");
        }
        std::env::remove_var("OPENAI_API_KEY");
        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    // -- headless runner (no real CLIs required) ------------------------------

    /// A throwaway adapter whose binary is a script we control.
    fn test_adapter(binary: &'static str, parse: ParseOutput) -> CliAgentSpec {
        CliAgentSpec {
            provider_id: "cli_agent/test",
            binary,
            label: "Test",
            install_hint: "-",
            version_argv: &["--version"],
            auth_probe_argv: None,
            extra_env: &[],
            experimental: false,
            denied_extra_args: &[],
            build_argv: |_, _| vec![],
            parse_output: parse,
            check_auth: None,
        }
    }

    /// Write a throwaway "agent binary": a `.cmd` script on Windows (exactly
    /// how `codex.cmd`/`claude` appear on PATH — a batch shim), an `sh`
    /// script elsewhere.
    #[cfg(windows)]
    fn script_tool(dir: &Path, name: &str, body: &str) -> PathBuf {
        let script = dir.join(format!("{name}.cmd"));
        std::fs::write(&script, body).unwrap();
        script
    }

    #[cfg(unix)]
    fn script_tool(dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let script = dir.join(name);
        std::fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    fn echo_tool(dir: &Path) -> PathBuf {
        echo_tool_named(dir, "fakeagent")
    }

    /// An echo tool whose file name matches `name` — needed when the test
    /// goes through a real adapter (binary-path overrides must match the
    /// adapter's stem).
    fn echo_tool_named(dir: &Path, name: &str) -> PathBuf {
        #[cfg(windows)]
        let body = "@echo canned response";
        #[cfg(unix)]
        let body = "echo canned response";
        script_tool(dir, name, body)
    }

    #[tokio::test]
    async fn run_headless_captures_stdout() {
        let dir = tempfile::tempdir().unwrap();
        let binary = echo_tool(dir.path());
        let spec = test_adapter("fakeagent", parse_plain_text);
        let out = run_headless(&binary, &[], &spec, None, Duration::from_secs(30))
            .await
            .unwrap();
        assert_eq!(out.exit_code, Some(0));
        assert!(String::from_utf8_lossy(&out.stdout).contains("canned response"));
    }

    #[tokio::test]
    async fn run_headless_timeout_kills_child() {
        let dir = tempfile::tempdir().unwrap();
        // The script spawns a long-lived grandchild (`ping`/`sleep`), like a
        // real `.cmd` shim launching node — killing only the top-level child
        // would leave the grandchild holding our pipes open.
        #[cfg(windows)]
        let binary = script_tool(dir.path(), "sleeper", "@ping -n 60 127.0.0.1 >nul");
        #[cfg(unix)]
        let binary = script_tool(dir.path(), "sleeper", "sleep 60");
        let spec = test_adapter("fakeagent", parse_plain_text);
        let started = Instant::now();
        let result = run_headless(&binary, &[], &spec, None, Duration::from_secs(1)).await;
        assert!(matches!(result, Err(LlmError::Timeout)));
        assert!(started.elapsed() < Duration::from_secs(30));
    }

    #[tokio::test]
    async fn run_headless_caps_stdout_but_drains() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("big.txt"), "a".repeat(MAX_STDOUT_BYTES * 2)).unwrap();
        #[cfg(windows)]
        let binary = script_tool(dir.path(), "dump", "@type \"%~dp0big.txt\"");
        #[cfg(unix)]
        let binary = script_tool(dir.path(), "dump", "cat \"$(dirname \"$0\")/big.txt\"");
        let spec = test_adapter("fakeagent", parse_plain_text);
        let out = run_headless(&binary, &[], &spec, None, Duration::from_secs(30))
            .await
            .unwrap();
        assert!(out.stdout_truncated);
        assert!(out.stdout.len() <= MAX_STDOUT_BYTES);
    }

    #[tokio::test]
    async fn run_headless_feeds_stdin() {
        let dir = tempfile::tempdir().unwrap();
        // Script echoes its stdin back. `findstr .*` matches every line —
        // an inline `cmd /c` equivalent would hit cmd.exe's quote-collapsing.
        #[cfg(windows)]
        let binary = script_tool(dir.path(), "parrot", "@findstr .*");
        #[cfg(unix)]
        let binary = script_tool(dir.path(), "parrot", "cat");
        let spec = test_adapter("fakeagent", parse_plain_text);
        let out = run_headless(
            &binary,
            &[],
            &spec,
            Some("the-prompt-payload"),
            Duration::from_secs(30),
        )
        .await
        .unwrap();
        assert!(String::from_utf8_lossy(&out.stdout).contains("the-prompt-payload"));
    }

    #[tokio::test]
    async fn provider_complete_round_trips_through_a_script() {
        let dir = tempfile::tempdir().unwrap();
        // The fixture must be named after the adapter's binary — the
        // override stem check rejects anything else.
        let binary = echo_tool_named(dir.path(), "codex");
        let spec = adapter_for("cli_agent/codex").unwrap();
        let mut cfg = config();
        cfg.binary_path = Some(binary.to_string_lossy().to_string());
        let provider = CliAgentProvider::new(
            // Point the real codex adapter at the fake binary; argv is
            // ignored by the canned script.
            spec,
            cfg,
            String::new(),
        );
        let response = provider.complete(req("", "hi")).await.unwrap();
        assert_eq!(response.text, "canned response");
        assert_eq!(response.model, "codex");
    }

    #[tokio::test]
    async fn provider_complete_refuses_denied_extra_args_at_spawn() {
        let dir = tempfile::tempdir().unwrap();
        let binary = echo_tool_named(dir.path(), "codex");
        let spec = adapter_for("cli_agent/codex").unwrap();
        let mut cfg = config();
        cfg.binary_path = Some(binary.to_string_lossy().to_string());
        // Simulates a config that bypassed `cli_agent_update_config`
        // (hand-edited store).
        cfg.extra_args = vec!["--sandbox".to_string(), "danger-full-access".to_string()];
        let provider = CliAgentProvider::new(spec, cfg, String::new());
        match provider.complete(req("", "hi")).await {
            Err(LlmError::Provider(msg)) => assert!(msg.contains("not allowed")),
            other => panic!("expected Provider error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn provider_complete_refuses_invalid_model_at_spawn() {
        let dir = tempfile::tempdir().unwrap();
        let binary = echo_tool_named(dir.path(), "codex");
        let spec = adapter_for("cli_agent/codex").unwrap();
        let mut cfg = config();
        cfg.binary_path = Some(binary.to_string_lossy().to_string());
        let provider = CliAgentProvider::new(spec, cfg, "model;rm -rf".to_string());
        assert!(matches!(
            provider.complete(req("", "hi")).await,
            Err(LlmError::Provider(_))
        ));
    }

    #[tokio::test]
    async fn provider_complete_errors_on_truncated_unparseable_output() {
        let dir = tempfile::tempdir().unwrap();
        // Emit more than MAX_STDOUT_BYTES of unparseable JSON-ish text; the
        // cap cuts it mid-stream and the parse must fail, not surface raw
        // partial JSON as the completion.
        std::fs::write(
            dir.path().join("big.txt"),
            format!("{{\"result\":\"{}\"", "a".repeat(MAX_STDOUT_BYTES * 2)),
        )
        .unwrap();
        #[cfg(windows)]
        let binary = script_tool(dir.path(), "claude", "@type \"%~dp0big.txt\"");
        #[cfg(unix)]
        let binary = script_tool(dir.path(), "claude", "cat \"$(dirname \"$0\")/big.txt\"");
        let spec = adapter_for("cli_agent/claude").unwrap();
        let mut cfg = config();
        cfg.binary_path = Some(binary.to_string_lossy().to_string());
        let provider = CliAgentProvider::new(spec, cfg, String::new());
        match provider.complete(req("", "hi")).await {
            Err(LlmError::Provider(msg)) => assert!(msg.contains("truncated")),
            other => panic!("expected truncation Provider error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn experimental_adapters_refuse_to_run() {
        for id in ["cli_agent/cursor_agent", "cli_agent/devin"] {
            let dir = tempfile::tempdir().unwrap();
            let spec = adapter_for(id).unwrap();
            assert!(spec.experimental, "{id} must be marked experimental");
            // Even with a real-looking binary override, complete() refuses.
            let binary = echo_tool_named(dir.path(), spec.binary);
            let mut cfg = config();
            cfg.binary_path = Some(binary.to_string_lossy().to_string());
            let provider = CliAgentProvider::new(spec, cfg, String::new());
            match provider.complete(req("", "hi")).await {
                Err(LlmError::Provider(msg)) => assert!(msg.contains("non-mutating")),
                other => panic!("{id}: expected Provider error, got {other:?}"),
            }
            let report = provider.health_check().await.unwrap();
            assert!(!report.ok);
        }
    }

    #[test]
    fn validate_config_rejects_enabling_experimental_adapters() {
        for id in ["cli_agent/cursor_agent", "cli_agent/devin"] {
            let spec = adapter_for(id).unwrap();
            let mut cfg = config();
            cfg.enabled = true;
            assert!(validate_config(spec, &cfg).is_err());
            cfg.enabled = false;
            assert!(validate_config(spec, &cfg).is_ok());
        }
    }

    #[tokio::test]
    async fn provider_reports_missing_binary() {
        let spec = adapter_for("cli_agent/codex").unwrap();
        let mut cfg = config();
        cfg.binary_path = Some("C:\\no\\such\\codex.exe".to_string());
        let provider = CliAgentProvider::new(spec, cfg, String::new());
        assert!(matches!(
            provider.complete(req("", "hi")).await,
            Err(LlmError::Provider(_))
        ));
        let report = provider.health_check().await.unwrap();
        assert!(!report.ok);
    }

    #[test]
    fn enabled_config_defaults_to_true() {
        assert!(CliAgentConfig::default().enabled);
    }

    // -- diagnostics redaction ------------------------------------------------

    #[test]
    fn sanitize_detail_redacts_cli_token_shapes() {
        for secret in [
            "error: ghp_0123456789abcdefghijABCDEFGH",
            "error: github_pat_11AAAAAA0123456789abcdefghijklmnop",
            "error: sk-ant-api03-AbCdEfGhIjKl",
            "error: AIzaSyB1234567890abcdefghijklmnopq",
            "error: xoxb-123456-abcdef",
            "error: eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0.abcDEF123_-xyz",
        ] {
            let out = sanitize_detail(secret);
            assert!(out.contains("[REDACTED]"), "{secret} was not redacted");
            assert!(
                !out.contains(secret.trim_start_matches("error: ")),
                "{secret} leaked through"
            );
        }
    }

    #[test]
    fn sanitize_detail_strips_home_and_username_paths() {
        let home = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_default();
        if !home.is_empty() {
            let out = sanitize_detail(&format!("config at {home}\\cli\\cfg.json failed"));
            assert!(!out.contains(&home), "home path leaked: {out}");
            assert!(out.contains('~'));
        }
        // Foreign-user paths are scrubbed even without env knowledge.
        let out = sanitize_detail(r"cannot read C:\Users\someoneelse\.codex\auth.json");
        assert!(!out.contains("someoneelse"), "username leaked: {out}");
    }

    // -- timeout clamp --------------------------------------------------------

    #[test]
    fn effective_timeout_is_clamped_to_the_cap() {
        let spec = adapter_for("cli_agent/codex").unwrap();
        let mut cfg = config();
        cfg.timeout_secs = Some(u64::MAX);
        let provider = CliAgentProvider::new(spec, cfg, String::new());
        let r = req("", "hi");
        assert_eq!(
            provider.effective_timeout(&r),
            Duration::from_secs(MAX_TIMEOUT_SECS)
        );

        let mut cfg = config();
        cfg.timeout_secs = Some(0);
        let provider = CliAgentProvider::new(spec, cfg, String::new());
        // 0 keeps the established "unset" semantics → caller's budget.
        assert_eq!(provider.effective_timeout(&r), r.timeout);

        let provider = CliAgentProvider::new(spec, config(), String::new());
        assert_eq!(provider.effective_timeout(&r), r.timeout);
    }
}
