//! Binary resolution (FR-012-02), the sanitized child environment
//! (NFR-012-02) and headless process execution with process-tree teardown.

use super::validate::validate_binary_override;
use super::{CliAgentSpec, HeadlessOutput, MAX_STDOUT_BYTES};
use crate::llm::types::LlmError;
use crate::settings::CliAgentConfig;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;

/// stderr is diagnostic-only: summarized, redacted, and capped before it can
/// land in an `LlmError::Provider` message or a log line.
const MAX_STDERR_BYTES: usize = 8 * 1024;
const MAX_ERROR_DETAIL_CHARS: usize = 300;

/// After the child exits, its pipes may still be held open by a surviving
/// grandchild (`.cmd` shim → node). Give the drain tasks a short grace
/// period, then abandon them rather than hanging the whole call.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------
// Binary detection (FR-012-02)
// ---------------------------------------------------------------------------

/// Extensions CreateProcess can run without a shell (`.cmd`/`.bat` go through
/// `cmd.exe` internally via std's batch handling). Extensionless shell
/// scripts and `.ps1` files are deliberately skipped — the first cannot be
/// spawned by CreateProcess, the second needs powershell.
#[cfg(windows)]
pub(crate) const SPAWNABLE_EXTENSIONS: &[&str] = &[".com", ".exe", ".bat", ".cmd"];

/// Candidate filenames for `binary` on this platform. Windows tries the
/// PATHEXT variants (filtered to spawnable extensions, honoring the user's
/// PATHEXT order); Unix wants the exact name.
#[cfg(windows)]
pub(crate) fn candidate_names(binary: &str, pathext: &[String]) -> Vec<String> {
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
pub(crate) fn candidate_names(binary: &str, _pathext: &[String]) -> Vec<String> {
    vec![binary.to_string()]
}

pub(crate) fn is_executable_file(path: &Path) -> bool {
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
pub(crate) fn find_on_path(binary: &str, path_var: &OsStr, pathext: &[String]) -> Option<PathBuf> {
    std::env::split_paths(path_var)
        .flat_map(|dir| {
            candidate_names(binary, pathext)
                .into_iter()
                .map(move |n| dir.join(n))
        })
        .find(|p| is_executable_file(p))
}

#[cfg(windows)]
pub(crate) fn pathext() -> Vec<String> {
    std::env::var("PATHEXT")
        .unwrap_or_default()
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(not(windows))]
pub(crate) fn pathext() -> Vec<String> {
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

pub(crate) fn sanitized_env(extra: &[&str]) -> Vec<(OsString, OsString)> {
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
pub(crate) fn sanitize_detail(raw: &str) -> String {
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

/// Sync variant for `Drop` — the cancel path aborts the `run_headless`
/// future, which tears down without an await point, so `taskkill`/`kill`
/// are invoked synchronously. Both calls are fast local operations.
fn kill_process_tree_sync(child: &mut tokio::process::Child) {
    #[cfg(windows)]
    if let Some(pid) = child.id() {
        let _ = std::process::Command::new(system_tool("taskkill.exe"))
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .output();
    }
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    // The direct child too — a no-op when the tree-kill already got it.
    let _ = child.start_kill();
}

/// RAII guard: while armed, dropping it kills the whole spawned process
/// tree. The cancel path aborts the `run_headless` future — without this
/// guard the drop would rely on `kill_on_drop`, which only reaches the
/// direct child and orphans the `.cmd` shim's `cmd.exe → node.exe`
/// grandchildren mid-call (AC-012-05). Stays armed through the success path
/// as well: a shim that exits but leaves a grandchild holding our pipes is
/// torn down with it.
struct ChildTreeGuard {
    child: tokio::process::Child,
    armed: bool,
}

impl ChildTreeGuard {
    fn new(child: tokio::process::Child) -> Self {
        Self { child, armed: true }
    }
}

impl std::ops::Deref for ChildTreeGuard {
    type Target = tokio::process::Child;
    fn deref(&self) -> &Self::Target {
        &self.child
    }
}

impl std::ops::DerefMut for ChildTreeGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.child
    }
}

impl Drop for ChildTreeGuard {
    fn drop(&mut self) {
        if self.armed {
            kill_process_tree_sync(&mut self.child);
        }
    }
}

/// Spawn `binary argv…` with a sanitized environment, an optional stdin
/// payload, and a hard timeout that kills the child (NFR-012-02). The
/// working directory is the temp dir — never the repo — so even a
/// misconfigured sandbox cannot touch the project's files, and codex's
/// read-only sandbox scopes reads to a throwaway tree.
pub(crate) async fn run_headless(
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
        // Belt under the ChildTreeGuard: the guard's Drop kills the whole
        // tree; this covers the direct child even then.
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

    // Wrapped immediately: an abort (assistant cancel, AC-012-05) drops this
    // future mid-await and the guard's Drop is what kills the process tree —
    // a plain kill_on_drop would orphan the shim's grandchildren.
    let mut child = ChildTreeGuard::new(cmd.spawn().map_err(|e| {
        log::warn!(
            "cli_agent {}: failed to spawn '{}': {e}",
            spec.provider_id,
            spec.binary
        );
        LlmError::Provider(format!("could not start the '{}' CLI", spec.binary))
    })?);

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
            child.armed = false; // explicitly reaped — Drop stays quiet
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
