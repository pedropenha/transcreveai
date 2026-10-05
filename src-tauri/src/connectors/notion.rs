//! Fixed read mapping for Notion's official MCP. No generic tool or REST fallback.
use super::{policy, types::*};
use rmcp::{
    model::CallToolRequestParams,
    transport::{
        streamable_http_client::StreamableHttpClientTransportConfig, StreamableHttpClientTransport,
    },
    ServiceExt,
};
use serde_json::{json, Value};
use std::time::Duration;
pub async fn read(
    config: &ConnectorConfig,
    token: &str,
    id: &str,
) -> ConnectorResult<RemoteDocument> {
    let id = policy::require_page(config, id)?;
    let client = reqwest13::Client::builder()
        .redirect(reqwest13::redirect::Policy::none())
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|_| ConnectorError::new(ConnectorErrorCode::TemporaryFailure))?;
    let transport = StreamableHttpClientTransport::with_client(
        client,
        StreamableHttpClientTransportConfig::with_uri("https://mcp.notion.com/mcp")
            .auth_header(token)
            .max_sse_event_size(128 * 1024)
            .max_concurrent_requests(1)
            .reinit_on_expired_session(false),
    );
    let service = ()
        .serve(transport)
        .await
        .map_err(|_| ConnectorError::new(ConnectorErrorCode::TemporaryFailure))?;
    let tools = service
        .list_tools(Default::default())
        .await
        .map_err(|_| ConnectorError::new(ConnectorErrorCode::TemporaryFailure))?;
    let fetch = tools
        .tools
        .iter()
        .find(|t| t.name == "notion-fetch")
        .ok_or_else(|| ConnectorError::new(ConnectorErrorCode::UnsupportedCapability))?;
    // Schema must declare id; a changed provider mapping fails closed.
    let schema = Value::Object((*fetch.input_schema).clone());
    if schema["properties"].get("id").is_none() {
        return Err(ConnectorError::new(
            ConnectorErrorCode::UnsupportedCapability,
        ));
    }
    let arguments = json!({"id":id})
        .as_object()
        .cloned()
        .ok_or_else(|| ConnectorError::new(ConnectorErrorCode::InvalidSchema))?;
    let response = service
        .call_tool(CallToolRequestParams::new("notion-fetch").with_arguments(arguments))
        .await
        .map_err(|_| ConnectorError::new(ConnectorErrorCode::TemporaryFailure))?;
    let _ = service.cancel().await;
    if response.is_error == Some(true) {
        return Err(ConnectorError::new(ConnectorErrorCode::ResourceUnavailable));
    }
    let value = serde_json::to_value(&response)
        .map_err(|_| ConnectorError::new(ConnectorErrorCode::InvalidSchema))?;
    let mut text = String::new();
    if let Some(content) = value["content"].as_array() {
        for block in content {
            if block["type"] == "text" {
                if let Some(t) = block["text"].as_str() {
                    if text.len() + t.len() > 32 * 1024 {
                        return Err(ConnectorError::new(ConnectorErrorCode::PayloadTooLarge));
                    }
                    text.push_str(t);
                }
            }
        }
    }
    // Root-only lookup: never follow retrieved links or expose broad search.
    // Provider fetch payload may include links but each later read rechecks UUID scope.
    Ok(RemoteDocument {
        connection_id: config.id.clone(),
        id: id.clone(),
        url: format!("https://www.notion.so/{}", id.replace('-', "")),
        title: "".into(),
        text,
        revision: None,
        truncated: false,
    })
}
