//! Bounded REST/OAuth responses; redirects never forward credentials.
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
pub async fn json(request: reqwest::RequestBuilder) -> ConnectorResult<Value> {
    let response = request.send().await.map_err(|e| {
        ConnectorError::new(if e.is_timeout() {
            ConnectorErrorCode::Timeout
        } else {
            ConnectorErrorCode::TemporaryFailure
        })
    })?;
    let status = response.status();
    if !status.is_success() {
        return Err(ConnectorError::new(match status.as_u16() {
            401 => ConnectorErrorCode::AuthInvalid,
            403 => ConnectorErrorCode::PermissionDenied,
            404 => ConnectorErrorCode::ResourceUnavailable,
            429 => ConnectorErrorCode::RateLimited,
            _ => ConnectorErrorCode::TemporaryFailure,
        }));
    }
    serde_json::from_slice(&bounded_body(response).await?)
        .map_err(|_| ConnectorError::new(ConnectorErrorCode::InvalidSchema))
}
