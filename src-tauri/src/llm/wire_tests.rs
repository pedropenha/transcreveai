//! Wire-level tests for the `LlmProvider` impls and the retry policy, against
//! a local mock HTTP server (same `TcpListener` pattern as `llm_client`'s
//! tests — no extra dev-dependency). Asserts on the request line, auth
//! headers and JSON body each impl produces, plus error classification and
//! the narrow-retry behavior.

use super::router::{self, LlmRoute, RetryPolicy};
use super::types::{LlmError, LlmMessage, LlmPurpose, LlmRequest, LlmRole};
use crate::settings::PostProcessProvider;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// One canned response per accepted connection; the raw requests are kept
/// for assertions.
struct MockServer {
    base_url: String,
    requests: Arc<Mutex<Vec<String>>>,
}

/// `responses[i]` answers connection `i`; the last one repeats if more
/// connections arrive.
fn serve(responses: Vec<(String, String)>) -> MockServer {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let requests: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&requests);

    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::from_std(listener).unwrap();
        let mut index = 0usize;
        while let Ok((mut stream, _)) = listener.accept().await {
            let (status, body) = responses
                .get(index)
                .or_else(|| responses.last())
                .cloned()
                .unwrap();
            index += 1;
            let captured = Arc::clone(&captured);
            tokio::spawn(async move {
                let raw = read_request(&mut stream).await;
                captured.lock().unwrap().push(raw);
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });

    MockServer {
        base_url: format!("http://{address}"),
        requests,
    }
}

/// Read one full HTTP/1.1 request (headers + `Content-Length` body).
async fn read_request(stream: &mut tokio::net::TcpStream) -> String {
    let mut buf = Vec::new();
    let mut chunk = [0_u8; 8192];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let read = tokio::time::timeout_at(deadline, stream.read(&mut chunk)).await;
        match read {
            Ok(Ok(0)) | Err(_) => break,
            Ok(Ok(n)) => {
                buf.extend_from_slice(&chunk[..n]);
                if let Some(end) = find_header_end(&buf) {
                    let headers = String::from_utf8_lossy(&buf[..end]);
                    let content_length = headers
                        .lines()
                        .find_map(|l| {
                            l.strip_prefix("Content-Length:")
                                .or_else(|| l.strip_prefix("content-length:"))
                                .and_then(|v| v.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if buf.len() >= end + 4 + content_length {
                        break;
                    }
                }
            }
            Ok(Err(_)) => break,
        }
    }
    String::from_utf8_lossy(&buf).to_string()
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn provider(id: &str, base_url: &str) -> PostProcessProvider {
    PostProcessProvider {
        id: id.to_string(),
        label: id.to_string(),
        base_url: base_url.to_string(),
        allow_base_url_edit: true,
        models_endpoint: Some("/models".to_string()),
        supports_structured_output: false,
    }
}

fn route(provider_id: &str, base_url: &str, model: &str) -> LlmRoute {
    LlmRoute {
        provider: provider(provider_id, base_url),
        model: model.to_string(),
        api_key: "test-key-material".to_string(),
        escalated: false,
    }
}

fn request(purpose: LlmPurpose) -> LlmRequest {
    LlmRequest {
        system: "You summarize meetings.".to_string(),
        messages: vec![LlmMessage {
            role: LlmRole::User,
            content: "Transcript goes here".to_string(),
        }],
        max_tokens: 512,
        temperature: 0.2,
        timeout: Duration::from_secs(5),
        purpose,
    }
}

fn last_request(server: &MockServer) -> String {
    server
        .requests
        .lock()
        .unwrap()
        .last()
        .cloned()
        .unwrap_or_default()
}

fn last_request_json(server: &MockServer) -> Value {
    let raw = last_request(server);
    let body = raw.split("\r\n\r\n").nth(1).unwrap_or("");
    serde_json::from_str(body).unwrap_or_else(|_| panic!("request body is not JSON: {raw}"))
}

#[tokio::test]
async fn openai_compat_posts_chat_completions_with_bearer() {
    let server = serve(vec![(
        "200 OK".to_string(),
        r###"{"choices":[{"message":{"role":"assistant","content":"## Resumo"}}],"usage":{"prompt_tokens":11,"completion_tokens":7}}"###
            .to_string(),
    )]);
    let provider = router::build_provider(&route("openai", &server.base_url, "gpt-4o-mini"))
        .expect("provider");

    let response = provider
        .complete(request(LlmPurpose::Summary))
        .await
        .expect("complete");

    assert_eq!(response.text, "## Resumo");
    assert_eq!(response.model, "gpt-4o-mini");
    assert_eq!(response.usage.input_tokens, Some(11));
    assert_eq!(response.usage.output_tokens, Some(7));

    let raw = last_request(&server);
    assert!(raw.starts_with("POST /chat/completions "), "{raw}");
    assert!(
        raw.contains("authorization: Bearer test-key-material"),
        "{raw}"
    );

    let body = last_request_json(&server);
    assert_eq!(body["model"], "gpt-4o-mini");
    assert_eq!(body["stream"], false);
    assert!((body["temperature"].as_f64().unwrap() - 0.2).abs() < 1e-6);
    // OpenAI uses max_completion_tokens, not the legacy max_tokens.
    assert_eq!(body["max_completion_tokens"], 512);
    assert!(body.get("max_tokens").is_none());
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(body["messages"][1]["role"], "user");
}

#[tokio::test]
async fn non_openai_compat_uses_max_tokens() {
    let server = serve(vec![(
        "200 OK".to_string(),
        r#"{"choices":[{"message":{"content":"ok"}}]}"#.to_string(),
    )]);
    let provider =
        router::build_provider(&route("groq", &server.base_url, "llama")).expect("provider");
    provider
        .complete(request(LlmPurpose::Summary))
        .await
        .expect("complete");
    let body = last_request_json(&server);
    assert_eq!(body["max_tokens"], 512);
    assert!(body.get("max_completion_tokens").is_none());
}

#[tokio::test]
async fn anthropic_posts_messages_with_api_key_headers() {
    let server = serve(vec![(
        "200 OK".to_string(),
        r#"{"content":[{"type":"text","text":"Resumo pronto"}],"usage":{"input_tokens":9,"output_tokens":4}}"#
            .to_string(),
    )]);
    let provider =
        router::build_provider(&route("anthropic", &server.base_url, "claude-haiku-4-5"))
            .expect("provider");

    let response = provider
        .complete(request(LlmPurpose::Summary))
        .await
        .expect("complete");

    assert_eq!(response.text, "Resumo pronto");
    assert_eq!(response.usage.input_tokens, Some(9));
    assert_eq!(response.usage.output_tokens, Some(4));

    let raw = last_request(&server);
    assert!(raw.starts_with("POST /messages "), "{raw}");
    assert!(raw.contains("x-api-key: test-key-material"), "{raw}");
    assert!(raw.contains("anthropic-version: 2023-06-01"), "{raw}");
    assert!(!raw.contains("authorization:"), "{raw}");

    let body = last_request_json(&server);
    assert_eq!(body["model"], "claude-haiku-4-5");
    assert_eq!(body["max_tokens"], 512);
    // Short system prompts stay a plain string.
    assert_eq!(body["system"], "You summarize meetings.");
    assert_eq!(body["messages"][0]["role"], "user");
}

#[tokio::test]
async fn anthropic_caches_long_system_prompts() {
    let server = serve(vec![(
        "200 OK".to_string(),
        r#"{"content":[{"type":"text","text":"ok"}]}"#.to_string(),
    )]);
    let provider =
        router::build_provider(&route("anthropic", &server.base_url, "m")).expect("provider");
    let mut req = request(LlmPurpose::Summary);
    req.system = "t".repeat(5_000);
    provider.complete(req).await.expect("complete");

    let body = last_request_json(&server);
    assert_eq!(body["system"][0]["type"], "text");
    assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
}

#[tokio::test]
async fn http_status_maps_to_error_taxonomy() {
    for (status, body, check) in [
        ("401 Unauthorized", "{}", "auth"),
        ("403 Forbidden", "{}", "auth"),
        ("500 Internal Server Error", "{}", "unavailable"),
        ("400 Bad Request", "{}", "provider"),
    ] {
        let server = serve(vec![(status.to_string(), body.to_string())]);
        let provider =
            router::build_provider(&route("openai", &server.base_url, "m")).expect("provider");
        let error = provider
            .complete(request(LlmPurpose::Summary))
            .await
            .unwrap_err();
        match check {
            "auth" => assert!(matches!(error, LlmError::Auth)),
            "unavailable" => assert!(matches!(error, LlmError::Unavailable(500))),
            _ => assert!(matches!(error, LlmError::Provider(_))),
        }
    }
}

#[tokio::test]
async fn rate_limit_carries_retry_after() {
    let server = serve(vec![(
        "429 Too Many Requests".to_string(),
        "{}".to_string(),
    )]);
    // Note: the mock server doesn't emit a Retry-After header; classification
    // of the header itself is covered in `http::tests`.
    let provider =
        router::build_provider(&route("openai", &server.base_url, "m")).expect("provider");
    let error = provider
        .complete(request(LlmPurpose::Summary))
        .await
        .unwrap_err();
    assert!(matches!(error, LlmError::RateLimited { .. }));
    assert!(error.is_transient());
}

#[tokio::test]
async fn request_timeout_maps_to_timeout_error() {
    // Server accepts and never answers.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::from_std(listener).unwrap();
        while let Ok((stream, _)) = listener.accept().await {
            // Hold the socket open without responding.
            tokio::spawn(async move {
                let _stream = stream;
                tokio::time::sleep(Duration::from_secs(30)).await;
            });
        }
    });

    let provider = router::build_provider(&route("openai", &format!("http://{address}"), "m"))
        .expect("provider");
    let mut req = request(LlmPurpose::Summary);
    req.timeout = Duration::from_millis(100);
    let error = provider.complete(req).await.unwrap_err();
    assert!(matches!(error, LlmError::Timeout), "got {error:?}");
}

#[tokio::test]
async fn retry_succeeds_after_transient_failure() {
    let server = serve(vec![
        ("500 Internal Server Error".to_string(), "{}".to_string()),
        (
            "200 OK".to_string(),
            r#"{"choices":[{"message":{"content":"done"}}]}"#.to_string(),
        ),
    ]);
    let provider =
        router::build_provider(&route("openai", &server.base_url, "m")).expect("provider");
    let policy = RetryPolicy {
        max_attempts: 3,
        base_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(5),
    };
    let response =
        router::complete_with_policy(provider.as_ref(), &request(LlmPurpose::Summary), policy)
            .await
            .expect("second attempt succeeds");
    assert_eq!(response.text, "done");
    assert_eq!(server.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn auth_failures_never_retry() {
    let server = serve(vec![("401 Unauthorized".to_string(), "{}".to_string())]);
    let provider =
        router::build_provider(&route("openai", &server.base_url, "m")).expect("provider");
    let policy = RetryPolicy {
        max_attempts: 3,
        base_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(5),
    };
    let error =
        router::complete_with_policy(provider.as_ref(), &request(LlmPurpose::Summary), policy)
            .await
            .unwrap_err();
    assert!(matches!(error, LlmError::Auth));
    assert_eq!(server.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn transient_failure_exhausts_attempts() {
    let server = serve(vec![(
        "503 Service Unavailable".to_string(),
        "{}".to_string(),
    )]);
    let provider =
        router::build_provider(&route("openai", &server.base_url, "m")).expect("provider");
    let policy = RetryPolicy {
        max_attempts: 2,
        base_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(5),
    };
    let error =
        router::complete_with_policy(provider.as_ref(), &request(LlmPurpose::Summary), policy)
            .await
            .unwrap_err();
    assert!(matches!(error, LlmError::Unavailable(503)));
    assert_eq!(server.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn health_check_uses_models_endpoint() {
    let server = serve(vec![(
        "200 OK".to_string(),
        r#"{"data":[{"id":"m"}]}"#.to_string(),
    )]);
    let provider =
        router::build_provider(&route("openai", &server.base_url, "m")).expect("provider");
    let report = provider.health_check().await.expect("health");
    assert!(report.ok);
    assert!(report.latency_ms.is_some());
    assert!(last_request(&server).starts_with("GET /models "));
}

#[tokio::test]
async fn health_check_maps_auth_failure() {
    let server = serve(vec![("401 Unauthorized".to_string(), "{}".to_string())]);
    let provider =
        router::build_provider(&route("openai", &server.base_url, "m")).expect("provider");
    let error = provider.health_check().await.unwrap_err();
    assert!(matches!(error, LlmError::Auth));
}

#[tokio::test]
async fn error_body_secrets_never_reach_the_error() {
    let server = serve(vec![(
        "400 Bad Request".to_string(),
        r#"{"error":"echo of bearer test-key-material header"}"#.to_string(),
    )]);
    let provider =
        router::build_provider(&route("openai", &server.base_url, "m")).expect("provider");
    let error = provider
        .complete(request(LlmPurpose::Summary))
        .await
        .unwrap_err();
    let rendered = error.to_string();
    assert!(
        !rendered.contains("bearer test-key-material"),
        "provider echo must be redacted: {rendered}"
    );
}
