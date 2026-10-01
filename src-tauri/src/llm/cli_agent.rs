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
//! * `cursor-agent` → `cursor-agent -p --output-format text` (best-effort —
//!   the binary is not installed on the dev machine; detection reports
//!   `ausente` until it is).
//! * `devin` → stub: no public headless CLI is documented; the adapter exists
//!   so the provider row shows up, detected as absent until a `devin` binary
//!   lands on PATH. `devin -p` with the prompt on stdin is the best guess.
//!
//! Multi-turn requests are flattened into a single delimited transcript —
//! these CLIs are stateless per invocation (FR-012-15 only re-sends history).
//!
//! Safety (NFR-012-02): argv array without a shell, an env whitelist instead
//! of inheritance, a neutral working directory (the temp dir — never the
//! repo), a timeout that `kill()`s the child, stdout capped at
//! [`MAX_STDOUT_CHARS`], stderr redacted + capped into [`LlmError`]. The
//! prompt, argv and response body are never logged — only the provider id,
//! exit status, latency and output sizes (T-003 policy).

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

type BuildArgv = fn(model: &str, extra_args: &[String]) -> Vec<String>;
type ParseOutput = fn(&[u8]) -> Result<(String, LlmUsage), LlmError>;
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

/// Plain-text stdout (claude/cursor/devin non-JSON paths): trim, reject
/// empty, cap.
fn parse_plain_text(stdout: &[u8]) -> Result<(String, LlmUsage), LlmError> {
    let text = String::from_utf8_lossy(stdout).trim().to_string();
    if text.is_empty() {
        return Err(LlmError::Provider(
            "CLI returned an empty completion".to_string(),
        ));
    }
    Ok((cap_text(text), LlmUsage::default()))
}

/// `codex exec --json` emits JSONL events; the answer is the concatenation
/// of `item.completed` events whose item is an `agent_message`. `turn.failed`
/// / top-level `error` events become the `LlmError::Provider` detail. When
/// nothing parses as JSONL at all the raw stdout is treated as plain text —
/// tolerance for older/newer codex builds.
fn parse_codex_jsonl(stdout: &[u8]) -> Result<(String, LlmUsage), LlmError> {
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
        return parse_plain_text(stdout);
    }
    if !messages.is_empty() {
        return Ok((cap_text(messages.join("\n")), usage));
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
/// Anything that does not parse as that object is treated as plain text.
fn parse_claude_json(stdout: &[u8]) -> Result<(String, LlmUsage), LlmError> {
    let text = String::from_utf8_lossy(stdout);
    let trimmed = text.trim();
    let Ok(parsed) = serde_json::from_str::<Value>(trimmed) else {
        return parse_plain_text(stdout);
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
        return Ok((cap_text(answer), usage));
    }

    parse_plain_text(stdout)
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
/// must point at an existing file (a stale override means "not detected" —
/// better than silently falling back to another install). Otherwise the
/// binary is searched on PATH (FR-012-02).
pub(crate) fn resolve_binary(spec: &CliAgentSpec, config: &CliAgentConfig) -> Option<PathBuf> {
    if let Some(override_path) = config
        .binary_path
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        return PathBuf::from(override_path)
            .is_file()
            .then(|| PathBuf::from(override_path));
    }
    std::env::var_os("PATH").and_then(|path| find_on_path(spec.binary, &path, &pathext()))
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
const ENV_WHITELIST: &[&str] = &[
    "PATH",
    "PATHEXT",
    "SYSTEMROOT",
    "WINDIR",
    "COMSPEC",
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

fn sanitized_env(extra: &[&str]) -> Vec<(OsString, OsString)> {
    ENV_WHITELIST
        .iter()
        .chain(extra.iter())
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

/// Trim + redact + cap a diagnostic blob before it can reach an error
/// message or a log line (T-003 policy).
fn sanitize_detail(raw: &str) -> String {
    let redacted = crate::utils::redact_secret_patterns(raw.trim());
    redacted
        .chars()
        .take(MAX_ERROR_DETAIL_CHARS)
        .collect::<String>()
}

/// Kill the spawned process **tree**. `.cmd`/`.bat` shims go through
/// `cmd.exe`, so the real CLI is a grandchild (`codex.cmd` → `cmd.exe` →
/// `node.exe`); a bare `kill()` would orphan it mid-call and leave it holding
/// the captured pipes. `taskkill /T` walks the tree. On unix the CLI binary
/// is the direct child, so `kill()` (SIGKILL) suffices.
async fn kill_process_tree(child: &mut tokio::process::Child) {
    #[cfg(windows)]
    if let Some(pid) = child.id() {
        let _ = tokio::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
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

    let mut child = cmd.spawn().map_err(|e| {
        log::warn!(
            "cli_agent {}: failed to spawn '{}': {e}",
            spec.provider_id,
            spec.binary
        );
        LlmError::Provider(format!("could not start the '{}' CLI", spec.binary))
    })?;

    // Feed the prompt on stdin from a task so a slow reader cannot block
    // the wait below; a broken pipe just means the child exited early.
    if let (Some(mut stdin), Some(payload)) = (child.stdin.take(), stdin_payload) {
        let payload = payload.to_string();
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            let _ = stdin.write_all(payload.as_bytes()).await;
            let _ = stdin.shutdown().await;
        });
    }

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

    let (stdout, stdout_truncated) = stdout_task.await.unwrap_or_default();
    let (stderr, _) = stderr_task.await.unwrap_or_default();

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

    /// Effective timeout: the per-provider override wins; otherwise the
    /// caller's budget (60 s assistant / 180 s summary map-reduce, FR-012-05).
    fn effective_timeout(&self, req: &LlmRequest) -> Duration {
        self.config
            .timeout_secs
            .filter(|t| *t > 0)
            .map(Duration::from_secs)
            .unwrap_or(req.timeout)
    }

    fn missing_binary_error(&self) -> LlmError {
        LlmError::Provider(format!(
            "the '{}' CLI was not found on PATH",
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
            let detail = sanitize_detail(&String::from_utf8_lossy(&out.stderr));
            let detail = if detail.is_empty() {
                format!(
                    "'{}' exited with code {:?}",
                    self.spec.binary, out.exit_code
                )
            } else {
                detail
            };
            return Err(LlmError::Provider(detail));
        }

        let (text, usage) = (self.spec.parse_output)(&out.stdout)?;
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
        let (text, usage) = parse_codex_jsonl(stdout.as_bytes()).unwrap();
        assert_eq!(text, "The answer.");
        assert_eq!(usage.input_tokens, Some(10));
        assert_eq!(usage.output_tokens, Some(4));
    }

    #[test]
    fn codex_jsonl_turn_failed_becomes_provider_error() {
        let stdout = r#"{"type":"turn.failed","error":{"message":"model exploded"}}"#;
        match parse_codex_jsonl(stdout.as_bytes()) {
            Err(LlmError::Provider(msg)) => assert!(msg.contains("model exploded")),
            other => panic!("expected Provider error, got {other:?}"),
        }
    }

    #[test]
    fn codex_jsonl_falls_back_to_plain_text() {
        let (text, _) = parse_codex_jsonl(b"  plain answer  ").unwrap();
        assert_eq!(text, "plain answer");
    }

    #[test]
    fn claude_json_reads_result_and_usage() {
        let stdout = r#"{"type":"result","is_error":false,"result":"hi there","usage":{"input_tokens":3,"output_tokens":2}}"#;
        let (text, usage) = parse_claude_json(stdout.as_bytes()).unwrap();
        assert_eq!(text, "hi there");
        assert_eq!(usage.input_tokens, Some(3));
    }

    #[test]
    fn claude_json_is_error_maps_to_provider_error() {
        let stdout = r#"{"type":"result","is_error":true,"result":"hit a limit"}"#;
        match parse_claude_json(stdout.as_bytes()) {
            Err(LlmError::Provider(msg)) => assert!(msg.contains("hit a limit")),
            other => panic!("expected Provider error, got {other:?}"),
        }
    }

    #[test]
    fn plain_text_rejects_empty_output() {
        assert!(matches!(
            parse_plain_text(b"   \n "),
            Err(LlmError::Provider(_))
        ));
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
        #[cfg(windows)]
        let body = "@echo canned response";
        #[cfg(unix)]
        let body = "echo canned response";
        script_tool(dir, "fakeagent", body)
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
        let binary = echo_tool(dir.path());
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
}
