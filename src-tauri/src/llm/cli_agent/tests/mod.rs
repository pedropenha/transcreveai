use super::adapters::*;
use super::parse::*;
use super::spawn::*;
use super::validate::*;

use super::*;
use crate::llm::types::{LlmMessage, LlmPurpose, LlmRole};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

mod adapters;
mod parse;
mod spawn;
mod validate;

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
fn spec(id: &str) -> &'static CliAgentSpec {
    adapter_for(id).unwrap()
}

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
