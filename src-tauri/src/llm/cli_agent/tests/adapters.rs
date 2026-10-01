use super::*;

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
