//! Public-client PKCE flow. Loopback exists only while an explicit login is pending.
use super::{http, types::*, vault::TokenBundle};
use oauth2::{
    basic::BasicClient, AuthUrl, ClientId, CsrfToken, PkceCodeChallenge, PkceCodeVerifier,
    RedirectUrl, Scope, TokenResponse, TokenUrl,
};
use serde_json::Value;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
pub const NOTION_RESOURCE: &str = "https://mcp.notion.com";
/// Built-in multi-tenant Entra public application maintained by the project.
/// A public client ID is not a credential; admins/users can still point a
/// connection at their own registration via `ConnectorInput::client_id`.
pub const AZURE_DEVOPS_DEFAULT_CLIENT_ID: &str = "4a3a52bb-fcb9-48ce-a172-3ba07a974d30";
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
pub struct Login {
    pub listener: TcpListener,
    pub authorization_url: String,
    pub state: String,
    pub verifier: PkceCodeVerifier,
    pub bundle: TokenBundle,
}
fn invalid() -> ConnectorError {
    ConnectorError::new(ConnectorErrorCode::InvalidSchema)
}
fn endpoint(value: &Value, field: &str) -> ConnectorResult<String> {
    let raw = value[field].as_str().ok_or_else(invalid)?;
    let url = reqwest::Url::parse(raw).map_err(|_| invalid())?;
    if url.scheme() != "https"
        || url.host_str() != Some("mcp.notion.com")
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ConnectorError::new(ConnectorErrorCode::PolicyDenied));
    }
    Ok(url.to_string())
}
pub async fn prepare(config: &mut ConnectorConfig) -> ConnectorResult<Login> {
    let old_port = config
        .oauth_redirect
        .as_deref()
        .and_then(|s| reqwest::Url::parse(s).ok())
        .and_then(|u| u.port())
        .unwrap_or(0);
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, old_port))
        .await
        .map_err(|_| ConnectorError::new(ConnectorErrorCode::Busy))?;
    let port = listener.local_addr().map_err(|_| invalid())?.port();
    let redirect = format!("http://127.0.0.1:{port}/oauth/callback");
    let (auth_url, token_url, resource, scopes) = match config.kind {
        ConnectorKind::Notion => {
            let client = http::client()?;
            let protected = http::json(
                client.get("https://mcp.notion.com/.well-known/oauth-protected-resource"),
            )
            .await?;
            if protected["resource"].as_str() != Some(NOTION_RESOURCE)
                || !protected["authorization_servers"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(NOTION_RESOURCE)))
            {
                return Err(ConnectorError::new(ConnectorErrorCode::PolicyDenied));
            }
            let metadata = http::json(
                client.get("https://mcp.notion.com/.well-known/oauth-authorization-server"),
            )
            .await?;
            if metadata["issuer"].as_str() != Some(NOTION_RESOURCE)
                || !metadata["code_challenge_methods_supported"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|v| v.as_str() == Some("S256")))
            {
                return Err(ConnectorError::new(
                    ConnectorErrorCode::UnsupportedCapability,
                ));
            }
            if config.client_id.is_none() {
                let registration = http::json(client.post(endpoint(&metadata,"registration_endpoint")?).json(&serde_json::json!({"client_name":"Transcreve.ai", "redirect_uris":[redirect], "grant_types":["authorization_code","refresh_token"], "response_types":["code"], "token_endpoint_auth_method":"none"}))).await?;
                // Only public-client registrations are accepted; no fallback global secret.
                if registration.get("client_secret").is_some()
                    || registration["token_endpoint_auth_method"]
                        .as_str()
                        .is_some_and(|s| s != "none")
                {
                    return Err(ConnectorError::new(
                        ConnectorErrorCode::UnsupportedCapability,
                    ));
                }
                config.client_id = Some(
                    registration["client_id"]
                        .as_str()
                        .filter(|s| !s.is_empty() && s.len() < 512)
                        .ok_or_else(invalid)?
                        .to_string(),
                );
            }
            (
                endpoint(&metadata, "authorization_endpoint")?,
                endpoint(&metadata, "token_endpoint")?,
                Some(NOTION_RESOURCE.to_string()),
                vec!["default"],
            )
        }
        ConnectorKind::AzureDevops => {
            let tenant = config.tenant.as_deref().unwrap_or("common");
            if tenant != "organizations" && tenant != "common" {
                super::policy::canonical_id(tenant)?;
            }
            (
                format!("https://login.microsoftonline.com/{tenant}/oauth2/v2.0/authorize"),
                format!("https://login.microsoftonline.com/{tenant}/oauth2/v2.0/token"),
                None,
                vec![
                    "499b84ac-1321-427f-aa17-267ca6975798/.default",
                    "offline_access",
                ],
            )
        }
    };
    let client_id = match config.kind {
        ConnectorKind::AzureDevops => config
            .client_id
            .clone()
            .unwrap_or_else(|| AZURE_DEVOPS_DEFAULT_CLIENT_ID.to_string()),
        _ => config.client_id.clone().ok_or_else(invalid)?,
    };
    let oauth = BasicClient::new(ClientId::new(client_id.clone()))
        .set_auth_uri(AuthUrl::new(auth_url.clone()).map_err(|_| invalid())?)
        .set_token_uri(TokenUrl::new(token_url.clone()).map_err(|_| invalid())?)
        .set_redirect_uri(RedirectUrl::new(redirect.clone()).map_err(|_| invalid())?);
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let mut request = oauth
        .authorize_url(CsrfToken::new_random)
        .set_pkce_challenge(challenge);
    for scope in scopes {
        request = request.add_scope(Scope::new(scope.to_string()));
    }
    if let Some(resource) = &resource {
        request = request.add_extra_param("resource", resource);
    }
    let (url, state) = request.url();
    config.oauth_redirect = Some(redirect.clone());
    Ok(Login {
        listener,
        authorization_url: url.to_string(),
        state: state.secret().clone(),
        verifier,
        bundle: TokenBundle {
            access_token: String::new(),
            refresh_token: None,
            expires_at: 0,
            client_id,
            auth_url,
            token_url,
            resource,
            redirect_uri: redirect,
        },
    })
}
pub fn parse_callback(
    request: &str,
    redirect: &str,
    expected_state: &str,
    issuer: Option<&str>,
) -> ConnectorResult<String> {
    let redirect = reqwest::Url::parse(redirect).map_err(|_| invalid())?;
    let mut lines = request.split("\r\n");
    let mut line = lines.next().ok_or_else(invalid)?.split(' ');
    if line.next() != Some("GET") {
        return Err(invalid());
    }
    let target = line.next().ok_or_else(invalid)?;
    if !target.starts_with("/oauth/callback?") || line.next() != Some("HTTP/1.1") {
        return Err(invalid());
    }
    let hosts: Vec<_> = lines
        .filter_map(|l| l.split_once(':'))
        .filter(|(k, _)| k.eq_ignore_ascii_case("host"))
        .map(|(_, v)| v.trim())
        .collect();
    let host = format!("127.0.0.1:{}", redirect.port().ok_or_else(invalid)?);
    if hosts.as_slice() != [host.as_str()] {
        return Err(ConnectorError::new(ConnectorErrorCode::PolicyDenied));
    }
    let url = redirect.join(target).map_err(|_| invalid())?;
    let pairs: Vec<_> = url.query_pairs().collect();
    let one = |key: &str| -> ConnectorResult<Option<String>> {
        let values: Vec<_> = pairs
            .iter()
            .filter(|(k, _)| k == key)
            .map(|(_, v)| v.to_string())
            .collect();
        if values.len() > 1 {
            return Err(invalid());
        }
        Ok(values.into_iter().next())
    };
    // oauth2's random state has 256 bits. Hash comparison avoids a variable-length secret comparison.
    if one("state")?
        .as_deref()
        .is_none_or(|s| !constant_state(s, expected_state))
    {
        return Err(ConnectorError::new(ConnectorErrorCode::AuthInvalid));
    }
    if let Some(received) = one("iss")? {
        if issuer != Some(received.as_str()) {
            return Err(ConnectorError::new(ConnectorErrorCode::AuthInvalid));
        }
    }
    if one("error")?.is_some() {
        return Err(ConnectorError::new(ConnectorErrorCode::PermissionDenied));
    }
    one("code")?
        .filter(|s| !s.is_empty() && s.len() <= 4096)
        .ok_or_else(invalid)
}
fn constant_state(a: &str, b: &str) -> bool {
    use sha2::{Digest, Sha256};
    Sha256::digest(a.as_bytes()) == Sha256::digest(b.as_bytes())
}
pub async fn finish(mut login: Login) -> ConnectorResult<TokenBundle> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
    let code = loop {
        let (mut socket, peer) = tokio::time::timeout_at(deadline, login.listener.accept())
            .await
            .map_err(|_| ConnectorError::new(ConnectorErrorCode::Timeout))?
            .map_err(|_| invalid())?;
        if !peer.ip().is_loopback() {
            continue;
        }
        let mut body = Vec::new();
        let mut chunk = [0u8; 1024];
        let read = async {
            loop {
                let n = socket.read(&mut chunk).await.map_err(|_| invalid())?;
                if n == 0 {
                    return Err(invalid());
                }
                body.extend_from_slice(&chunk[..n]);
                if body.len() > 8192 {
                    return Err(ConnectorError::new(ConnectorErrorCode::PayloadTooLarge));
                }
                if body.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let raw = std::str::from_utf8(&body).map_err(|_| invalid())?;
            parse_callback(
                raw,
                &login.bundle.redirect_uri,
                &login.state,
                login.bundle.resource.as_deref(),
            )
        };
        let result = tokio::time::timeout_at(
            deadline.min(tokio::time::Instant::now() + Duration::from_secs(5)),
            read,
        )
        .await;
        let accepted = matches!(&result, Ok(Ok(_)));
        let response = if accepted {
            "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        } else {
            "HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        };
        let _ = tokio::time::timeout(
            Duration::from_secs(1),
            socket.write_all(response.as_bytes()),
        )
        .await;
        match result {
            Ok(Ok(code)) => break code,
            Ok(Err(e)) if e.code == ConnectorErrorCode::PermissionDenied => return Err(e),
            _ => continue,
        }
    };
    drop(login.listener);
    let oauth = BasicClient::new(ClientId::new(login.bundle.client_id.clone()))
        .set_auth_uri(AuthUrl::new(login.bundle.auth_url.clone()).map_err(|_| invalid())?)
        .set_token_uri(TokenUrl::new(login.bundle.token_url.clone()).map_err(|_| invalid())?)
        .set_redirect_uri(
            RedirectUrl::new(login.bundle.redirect_uri.clone()).map_err(|_| invalid())?,
        );
    let mut exchange = oauth
        .exchange_code(oauth2::AuthorizationCode::new(code))
        .set_pkce_verifier(login.verifier);
    if let Some(resource) = &login.bundle.resource {
        exchange = exchange.add_extra_param("resource", resource);
    }
    let tokens = exchange
        .request_async(&bounded_oauth)
        .await
        .map_err(|_| ConnectorError::new(ConnectorErrorCode::AuthInvalid))?;
    login.bundle.access_token = tokens.access_token().secret().clone();
    login.bundle.refresh_token = tokens.refresh_token().map(|v| v.secret().clone());
    login.bundle.expires_at = now()
        + tokens
            .expires_in()
            .unwrap_or(Duration::from_secs(3600))
            .as_secs();
    Ok(login.bundle)
}
async fn bounded_oauth(request: oauth2::HttpRequest) -> ConnectorResult<oauth2::HttpResponse> {
    let client = http::client()?;
    let response = client
        .request(request.method().clone(), request.uri().to_string())
        .headers(request.headers().clone())
        .body(request.body().clone())
        .send()
        .await
        .map_err(|_| ConnectorError::new(ConnectorErrorCode::TemporaryFailure))?;
    let mut builder = oauth2::http::Response::builder().status(response.status());
    if let Some(headers) = builder.headers_mut() {
        *headers = response.headers().clone();
    }
    builder
        .body(http::bounded_body(response).await?)
        .map_err(|_| invalid())
}
pub async fn refresh(bundle: &mut TokenBundle) -> ConnectorResult<()> {
    let token = bundle
        .refresh_token
        .clone()
        .ok_or_else(|| ConnectorError::new(ConnectorErrorCode::AuthInvalid))?;
    let oauth = BasicClient::new(ClientId::new(bundle.client_id.clone()))
        .set_token_uri(TokenUrl::new(bundle.token_url.clone()).map_err(|_| invalid())?);
    let token = oauth2::RefreshToken::new(token);
    let mut exchange = oauth.exchange_refresh_token(&token);
    if let Some(resource) = &bundle.resource {
        exchange = exchange.add_extra_param("resource", resource);
    }
    let tokens = exchange.request_async(&bounded_oauth).await.map_err(|e| {
        use oauth2::{basic::BasicErrorResponseType, RequestTokenError};
        let code = match e {
            RequestTokenError::ServerResponse(ref r)
                if matches!(
                    r.error(),
                    BasicErrorResponseType::InvalidGrant | BasicErrorResponseType::InvalidClient
                ) =>
            {
                ConnectorErrorCode::AuthInvalid
            }
            _ => ConnectorErrorCode::TemporaryFailure,
        };
        ConnectorError::new(code)
    })?;
    bundle.access_token = tokens.access_token().secret().clone();
    if let Some(refresh) = tokens.refresh_token() {
        bundle.refresh_token = Some(refresh.secret().clone());
    }
    bundle.expires_at = now()
        + tokens
            .expires_in()
            .unwrap_or(Duration::from_secs(3600))
            .as_secs();
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn callback_rejects_wrong_host_state_path_and_duplicates() {
        let uri = "http://127.0.0.1:123/oauth/callback";
        let request="GET /oauth/callback?code=secret&state=expected HTTP/1.1\r\nHost: 127.0.0.1:123\r\n\r\n";
        assert_eq!(
            parse_callback(request, uri, "expected", None).unwrap(),
            "secret"
        );
        for bad in [
            request.replace("expected", "wrong"),
            request.replace("127.0.0.1:123", "evil:123"),
            request.replace("code=secret", "code=a&code=b"),
            request.replace("/oauth/callback", "/other"),
        ] {
            assert!(parse_callback(&bad, uri, "expected", None).is_err());
        }
    }
}
