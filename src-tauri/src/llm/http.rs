//! Shared HTTP transport for the `LlmProvider` implementations.
//!
//! One choke point for: per-request timeouts, error classification into
//! [`LlmError`] (auth / rate-limit / network / timeout — the input to the
//! router's narrow retry), and sanitization — URLs are logged without
//! credentials or query strings and provider error bodies pass through
//! `utils::redact_secret_patterns` before they can reach a log or an error
//! message (FR-011-03, T-003).

use super::types::LlmError;
use crate::llm_client::{create_client, report_reqwest_error, sanitized_url_for_log};
use crate::settings::PostProcessProvider;
use log::debug;
use reqwest::header::HeaderMap;
use reqwest::StatusCode;
use serde_json::Value;
use std::time::{Duration, Instant};

/// Bodies of failed responses are capped before they enter an error string —
/// provider errors sometimes echo part of the request.
const MAX_ERROR_BODY_CHARS: usize = 400;

/// What to send. `Post` carries a JSON body; `Get` is for `…/models` health
/// checks.
pub(crate) enum Request {
    Get,
    Post(Value),
}

pub(crate) struct HttpOutcome {
    pub status: StatusCode,
    pub body: Vec<u8>,
    pub latency_ms: u32,
}

/// Classify a transport failure into the `LlmError` taxonomy. The detail line
/// goes to the log via `report_reqwest_error` (bounded, sanitized); the
/// returned error itself never embeds the URL — `reqwest::Error`'s Display
/// would leak it, so it only lives behind `Network`'s `#[source]`.
fn classify_transport_error(context: &str, error: reqwest::Error) -> LlmError {
    let timed_out = error.is_timeout();
    report_reqwest_error(context, &error);
    if timed_out {
        LlmError::Timeout
    } else {
        LlmError::Network(error)
    }
}

/// `Retry-After` header → `Duration`. Only the delta-seconds form is honored;
/// HTTP-date and junk values degrade to `None` (caller falls back to its own
/// backoff).
fn parse_retry_after(headers: &HeaderMap) -> Option<Duration> {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
}

/// Map a non-2xx status into the taxonomy. The response body is redacted and
/// truncated before it lands in `Provider`'s message or in the debug log —
/// it can echo auth material (FR-011-03).
fn classify_http_error(status: StatusCode, headers: &HeaderMap, body: &[u8]) -> LlmError {
    let body_text = String::from_utf8_lossy(body);
    let redacted = crate::utils::redact_secret_patterns(&body_text);
    let snippet: String = redacted.chars().take(MAX_ERROR_BODY_CHARS).collect();
    debug!("LLM provider HTTP {status} body: {snippet}");

    match status.as_u16() {
        401 | 403 => LlmError::Auth,
        429 => LlmError::RateLimited {
            retry_after: parse_retry_after(headers),
        },
        s if (500..=599).contains(&s) => LlmError::Unavailable(s),
        s => {
            if snippet.is_empty() {
                LlmError::Provider(format!("HTTP {s}"))
            } else {
                LlmError::Provider(format!("HTTP {s}: {snippet}"))
            }
        }
    }
}

/// Send one request to `url` with the provider's auth headers and a hard
/// timeout. Success returns the raw body for the caller to parse; any failure
/// is already classified. Never logs the API key or the request body
/// (prompt bodies may carry meeting transcripts — T-003).
pub(crate) async fn send(
    provider: &PostProcessProvider,
    api_key: &str,
    request: Request,
    url: &str,
    timeout: Duration,
) -> Result<HttpOutcome, LlmError> {
    debug!("LLM request to {}", sanitized_url_for_log(url));

    let client = create_client(provider, api_key).map_err(LlmError::Provider)?;

    // `tokio::time::timeout` is the authoritative deadline: reqwest's
    // per-request timeout can surface as a canceled-request error (not
    // `is_timeout`) when the connection is mid-flight, which would leak out
    // as `Network` instead of `Timeout`.
    let exchange = async {
        let builder = match &request {
            Request::Get => client.get(url),
            Request::Post(body) => client.post(url).json(body),
        };
        let response = builder
            .send()
            .await
            .map_err(|e| classify_transport_error("LLM request failed", e))?;
        let status = response.status();
        let headers = response.headers().clone();
        let body = response
            .bytes()
            .await
            .map_err(|e| classify_transport_error("Failed to read LLM response", e))?;
        Ok::<_, LlmError>((status, headers, body))
    };

    let started = Instant::now();
    let (status, headers, body) = match tokio::time::timeout(timeout, exchange).await {
        Ok(result) => result?,
        Err(_) => {
            debug!("LLM request to {} timed out", sanitized_url_for_log(url));
            return Err(LlmError::Timeout);
        }
    };
    let latency_ms = started.elapsed().as_millis() as u32;

    debug!(
        "LLM response status {status} in {latency_ms} ms from {}",
        sanitized_url_for_log(url)
    );

    if !status.is_success() {
        return Err(classify_http_error(status, &headers, &body));
    }

    Ok(HttpOutcome {
        status,
        body: body.to_vec(),
        latency_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    #[test]
    fn retry_after_parses_delta_seconds_only() {
        let mut headers = HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, HeaderValue::from_static("7"));
        assert_eq!(parse_retry_after(&headers), Some(Duration::from_secs(7)));

        headers.insert(
            reqwest::header::RETRY_AFTER,
            HeaderValue::from_static("Wed, 21 Oct 2050 07:28:00 GMT"),
        );
        assert_eq!(parse_retry_after(&headers), None);

        headers.insert(
            reqwest::header::RETRY_AFTER,
            HeaderValue::from_static("soon"),
        );
        assert_eq!(parse_retry_after(&headers), None);
    }

    #[test]
    fn http_error_classification_is_secret_free() {
        let headers = HeaderMap::new();
        let body = br#"{"error":"bad key sk-proj-AbCdEf123456 provided"}"#;

        assert!(matches!(
            classify_http_error(StatusCode::UNAUTHORIZED, &headers, body),
            LlmError::Auth
        ));
        match classify_http_error(StatusCode::TOO_MANY_REQUESTS, &headers, body) {
            LlmError::RateLimited { retry_after } => assert_eq!(retry_after, None),
            other => panic!("expected RateLimited, got {other:?}"),
        }
        assert!(matches!(
            classify_http_error(StatusCode::BAD_GATEWAY, &headers, body),
            LlmError::Unavailable(502)
        ));
        match classify_http_error(StatusCode::BAD_REQUEST, &headers, body) {
            LlmError::Provider(msg) => {
                assert!(msg.contains("HTTP 400"));
                assert!(
                    !msg.contains("sk-proj"),
                    "provider error bodies must be redacted: {msg}"
                );
            }
            other => panic!("expected Provider, got {other:?}"),
        }
    }

    #[test]
    fn transient_classification_drives_retry() {
        assert!(LlmError::Timeout.is_transient());
        assert!(LlmError::Unavailable(503).is_transient());
        assert!(LlmError::RateLimited { retry_after: None }.is_transient());
        assert!(!LlmError::Auth.is_transient());
        assert!(!LlmError::MissingApiKey.is_transient());
        assert!(!LlmError::Offline.is_transient());
        assert!(!LlmError::Unsupported.is_transient());
        assert!(!LlmError::Provider("bad request".into()).is_transient());
    }
}
