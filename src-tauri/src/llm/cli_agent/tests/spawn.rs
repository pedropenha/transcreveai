use super::*;
use std::time::Instant;

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

/// AC-012-05: aborting the call (assistant cancel / Esc) must take the
/// whole process tree down — `kill_on_drop` alone only reaches the
/// direct child. The shim spawns a grandchild that keeps writing a
/// heartbeat file; if the tree dies, the file stops growing.
#[tokio::test]
async fn aborting_run_headless_kills_the_whole_tree() {
    let dir = tempfile::tempdir().unwrap();
    let hb = dir.path().join("hb.txt");
    #[cfg(windows)]
    let binary = script_tool(
        dir.path(),
        "treed",
        concat!(
            "@start /b cmd /c \"for /l %%i in (1,1,60000) do ",
            "(echo x>>\"%~dp0hb.txt\" & ping -n 2 127.0.0.1 >nul)\"\n",
            "@ping -n 6000 127.0.0.1 >nul"
        ),
    );
    #[cfg(unix)]
    let binary = script_tool(
        dir.path(),
        "treed",
        "( while :; do echo x >> \"$(dirname \"$0\")/hb.txt\"; sleep 0.2; done ) &\nsleep 60",
    );
    let spec = test_adapter("fakeagent", parse_plain_text);
    let task = tokio::spawn(async move {
        run_headless(&binary, &[], &spec, None, Duration::from_secs(60)).await
    });
    // Let the shim and the heartbeat grandchild actually start.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    task.abort();
    let _ = task.await;
    let size_at_abort = std::fs::metadata(&hb).map(|m| m.len()).unwrap_or(0);
    assert!(size_at_abort > 0, "grandchild never started its heartbeat");
    tokio::time::sleep(Duration::from_millis(1600)).await;
    let size_later = std::fs::metadata(&hb).map(|m| m.len()).unwrap_or(0);
    assert_eq!(
        size_later, size_at_abort,
        "the heartbeat grandchild survived the abort — the process tree was not killed"
    );
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
