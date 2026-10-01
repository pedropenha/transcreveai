//! Config hardening (NFR-012-04): model ids, binary overrides and
//! caller-supplied extra args are validated against a shared deny-list plus
//! each adapter's own additions — enforced at the update boundary *and*
//! re-checked at spawn so a hand-edited store cannot bypass it.

use super::spawn::{is_executable_file, SPAWNABLE_EXTENSIONS};
use super::{CliAgentSpec, MAX_EXTRA_ARGS, MAX_EXTRA_ARG_CHARS};
use crate::settings::CliAgentConfig;
use std::path::{Path, PathBuf};

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
