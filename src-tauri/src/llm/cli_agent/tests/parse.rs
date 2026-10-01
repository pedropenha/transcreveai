use super::*;

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
