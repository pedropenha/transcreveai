//! Explicit opt-in acceptance: synthetic content only, no API key/config writes.
use super::*;

async fn synthetic_summary(id: &str) {
    assert_eq!(
        std::env::var("TRANSCREVE_RUN_LIVE_CLI_TESTS").as_deref(),
        Ok("1"),
        "explicit live opt-in required"
    );
    let provider = CliAgentProvider::new(spec(id), config(), String::new());
    let health = provider.health_check().await.unwrap();
    assert!(health.ok, "installed CLI must be authenticated");
    let mut request = req(
        "Summarize this synthetic meeting in one sentence. No tools or files.",
        "Ana: We agreed to publish the demo on Friday. Bruno: I will prepare the slides.",
    );
    request.timeout = Duration::from_secs(120);
    let response = provider.complete(request).await.unwrap();
    assert!(!response.text.trim().is_empty());
    assert!(
        response.text.to_ascii_lowercase().contains("friday") || response.text.contains("sexta")
    );
}

#[tokio::test]
#[ignore = "requires explicit live CLI opt-in; uses subscription with synthetic text"]
async fn live_codex_subscription_summary() {
    synthetic_summary("cli_agent/codex").await;
}

#[tokio::test]
#[ignore = "requires explicit live CLI opt-in; uses subscription with synthetic text"]
async fn live_claude_subscription_summary() {
    synthetic_summary("cli_agent/claude").await;
}

#[tokio::test]
#[ignore = "requires explicit live CLI opt-in; starts and cancels a synthetic request"]
async fn live_codex_cancel_reaps_process() {
    assert_eq!(
        std::env::var("TRANSCREVE_RUN_LIVE_CLI_TESTS").as_deref(),
        Ok("1")
    );
    let adapter = spec("cli_agent/codex");
    let binary = resolve_binary(adapter, &config()).unwrap();
    let argv = codex_argv("", &[]);
    let working_dir = tempfile::tempdir().unwrap();
    let mut cmd = tokio::process::Command::new(binary);
    cmd.args(argv)
        .env_clear()
        .envs(sanitized_env(adapter.extra_env))
        .current_dir(working_dir.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = super::super::spawn::ChildTreeGuard::new(cmd.spawn().unwrap());
    let pid = child.id().unwrap();
    use tokio::io::AsyncWriteExt;
    let mut stdin = child.stdin.take().unwrap();
    stdin
        .write_all(b"Produce a long detailed fictional meeting summary of 5000 words. No tools.")
        .await
        .unwrap();
    stdin.shutdown().await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        child.try_wait().unwrap().is_none(),
        "request exited before cancellation"
    );
    drop(child);
    #[cfg(windows)]
    {
        let process = std::process::Command::new("tasklist.exe")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .output()
            .unwrap();
        assert!(!String::from_utf8_lossy(&process.stdout).contains(&format!("\"{pid}\"")));
    }
    #[cfg(unix)]
    assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
}
