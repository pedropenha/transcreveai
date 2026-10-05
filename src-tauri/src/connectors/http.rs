//! Bounded REST/OAuth responses; GET redirects are followed only within
//! first-party Azure DevOps hosts, so credentials never leave that set.
use super::types::*;
use futures_util::StreamExt;
use serde_json::Value;
use std::time::Duration;
pub const MAX_RESPONSE_BYTES: usize = 128 * 1024;
pub fn client() -> ConnectorResult<reqwest::Client> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30))
        .user_agent("Transcreve.ai-connectors/1")
        .build()
        .map_err(|_| ConnectorError::new(ConnectorErrorCode::TemporaryFailure))
}
pub async fn bounded_body(response: reqwest::Response) -> ConnectorResult<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|n| n > MAX_RESPONSE_BYTES as u64)
    {
        return Err(ConnectorError::new(ConnectorErrorCode::PayloadTooLarge));
    }
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| ConnectorError::new(ConnectorErrorCode::TemporaryFailure))?;
        if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err(ConnectorError::new(ConnectorErrorCode::PayloadTooLarge));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}
fn redirect_allowed(url: &reqwest::Url) -> bool {
    let host = url.host_str().unwrap_or_default();
    url.scheme() == "https"
        && (host == "dev.azure.com"
            || host.ends_with(".dev.azure.com")
            || host == "visualstudio.com"
            || host.ends_with(".visualstudio.com"))
}
pub async fn json(request: reqwest::RequestBuilder) -> ConnectorResult<Value> {
    let mut request = request
        .build()
        .map_err(|_| ConnectorError::new(ConnectorErrorCode::TemporaryFailure))?;
    let client = client()?;
    for _ in 0..4 {
        let response = client
            .execute(
                request
                    .try_clone()
                    .ok_or_else(|| ConnectorError::new(ConnectorErrorCode::TemporaryFailure))?,
            )
            .await
            .map_err(|e| {
                ConnectorError::new(if e.is_timeout() {
                    ConnectorErrorCode::Timeout
                } else {
                    ConnectorErrorCode::TemporaryFailure
                })
            })?;
        let status = response.status();
        if status.is_redirection() {
            if request.method() != reqwest::Method::GET {
                return Err(ConnectorError::new(ConnectorErrorCode::TemporaryFailure));
            }
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| ConnectorError::new(ConnectorErrorCode::TemporaryFailure))?;
            let url = request
                .url()
                .join(location)
                .map_err(|_| ConnectorError::new(ConnectorErrorCode::TemporaryFailure))?;
            if !redirect_allowed(&url) {
                return Err(ConnectorError::new(ConnectorErrorCode::TemporaryFailure));
            }
            *request.url_mut() = url;
            continue;
        }
        if !status.is_success() {
            return Err(ConnectorError::new(match status.as_u16() {
                401 => ConnectorErrorCode::AuthInvalid,
                403 => ConnectorErrorCode::PermissionDenied,
                404 => ConnectorErrorCode::ResourceUnavailable,
                429 => ConnectorErrorCode::RateLimited,
                _ => ConnectorErrorCode::TemporaryFailure,
            }));
        }
        return serde_json::from_slice(&bounded_body(response).await?)
            .map_err(|_| ConnectorError::new(ConnectorErrorCode::InvalidSchema));
    }
    Err(ConnectorError::new(ConnectorErrorCode::TemporaryFailure))
}
