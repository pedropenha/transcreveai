use super::*;

// -- extra_args validation ------------------------------------------------

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
    assert!(validate_extra_args(spec("cli_agent/codex"), &["--color=never".into()]).is_ok());
    assert!(validate_extra_args(spec("cli_agent/claude"), &["--effort=low".into()]).is_ok());
}

#[test]
fn extra_args_reject_unknown_flags_positionals_and_short_flag_clusters() {
    for id in ["cli_agent/codex", "cli_agent/claude"] {
        for arg in [
            "--",
            "resume",
            "-xsread-only",
            "--enable=hooks",
            "--plugin-dir=x",
            "--debug-file=x",
            "--search",
            "--verbose",
            "--no-session-persistence=false",
        ] {
            assert!(
                validate_extra_args(spec(id), &[arg.into()]).is_err(),
                "{id}: {arg}"
            );
        }
    }
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
    let max_ok = vec!["--color=never".to_string(); MAX_EXTRA_ARGS];
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
