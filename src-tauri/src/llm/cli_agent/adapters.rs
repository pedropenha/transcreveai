//! Per-tool adapter table (FR-012-01) — see the module docs in `super` for
//! the flag rationale per CLI.

use super::parse::{
    check_auth_claude, check_auth_exit_code, parse_claude_json, parse_codex_jsonl, parse_plain_text,
};
use super::CliAgentSpec;

// ---------------------------------------------------------------------------
// Adapter table
// ---------------------------------------------------------------------------

pub(crate) fn codex_argv(model: &str, extra_args: &[String]) -> Vec<String> {
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

pub(crate) fn claude_argv(model: &str, extra_args: &[String]) -> Vec<String> {
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

pub(crate) fn cursor_agent_argv(model: &str, extra_args: &[String]) -> Vec<String> {
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

pub(crate) fn devin_argv(_model: &str, extra_args: &[String]) -> Vec<String> {
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
