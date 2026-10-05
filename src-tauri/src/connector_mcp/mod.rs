//! Read-only MCP facade. OAuth credentials never leave the connector core.
//! Local pairing credentials authorize a bounded, revocable connection scope.
mod ipc;
mod policy;
mod server;
mod stdio;

use crate::connectors::{self, types::ConnectorKind};
use crate::secrets;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use specta::Type;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager};
use tauri_plugin_store::StoreExt;
use tokio::sync::Mutex;

pub use stdio::run_stdio;
const MAX_GRANTS: usize = 32;
const MAX_CONNECTIONS: usize = 32;
const GRANT_TTL: u64 = 30 * 24 * 60 * 60;
const STORE_NAME: &str = "connector-mcp.json";
pub(super) const DEADLINE: Duration = Duration::from_secs(45);

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct McpGrant {
    pub client_id: String,
    pub label: String,
    pub connection_ids: Vec<String>,
    pub expires_at: u64,
    pub revoked: bool,
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredGrant {
    grant: McpGrant,
    revisions: Vec<u32>,
}

// No Debug: this bundle contains the local bearer credential, not OAuth tokens.
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct PairCredential {
    token: String,
    endpoint: String,
    server_pid: u32,
}

#[derive(Debug, Clone, Serialize, Type)]
pub struct McpStatus {
    pub running: bool,
    pub grants: Vec<McpGrant>,
}

#[derive(Debug, Clone, Serialize, Type)]
pub struct McpConfiguration {
    pub command: String,
    pub args: Vec<String>,
    pub transport: String,
}

#[derive(Default)]
pub struct ConnectorMcpState {
    mutation: Mutex<()>,
    runtime: Mutex<Option<server::LocalServer>>,
}

pub(super) type BridgeResult<T> = Result<T, String>;

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|v| v.as_secs())
        .unwrap_or(u64::MAX)
}

fn key(id: &str) -> BridgeResult<String> {
    let id = uuid::Uuid::parse_str(id).map_err(|_| "invalid_client".to_owned())?;
    Ok(format!("mcp-client-{id}"))
}

fn load_grants(app: &AppHandle) -> BridgeResult<Vec<StoredGrant>> {
    let store = app
        .store(crate::portable::store_path(STORE_NAME))
        .map_err(|_| "store_unavailable")?;
    let Some(value) = store.get("grants") else {
        return Ok(Vec::new());
    };
    let grants: Vec<StoredGrant> = serde_json::from_value(value).map_err(|_| "invalid_grants")?;
    if grants.len() > MAX_GRANTS
        || grants.iter().any(|g| {
            g.grant.connection_ids.len() > MAX_CONNECTIONS
                || g.grant.connection_ids.len() != g.revisions.len()
        })
    {
        return Err("invalid_grants".into());
    }
    Ok(grants)
}

fn save_grants(app: &AppHandle, grants: &[StoredGrant]) -> BridgeResult<()> {
    let store = app
        .store(crate::portable::store_path(STORE_NAME))
        .map_err(|_| "store_unavailable")?;
    let value = serde_json::to_value(grants).map_err(|_| "invalid_grants")?;
    let old = store.get("grants");
    store.set("grants", value);
    if store.save().is_err() {
        if let Some(old) = old {
            store.set("grants", old);
        } else {
            store.delete("grants");
        }
        return Err("store_unavailable".into());
    }
    Ok(())
}

fn load_credential(id: &str) -> BridgeResult<PairCredential> {
    let value = secrets::secret_store()
        .get(&key(id)?)
        .map_err(|_| "vault_unavailable")?
        .ok_or("pairing_required")?;
    serde_json::from_str(&value).map_err(|_| "pairing_required".into())
}

fn save_credential(id: &str, credential: &PairCredential) -> BridgeResult<()> {
    let value = serde_json::to_string(credential).map_err(|_| "vault_unavailable")?;
    secrets::secret_store()
        .set(&key(id)?, &value)
        .map_err(|_| "vault_unavailable".into())
}

async fn ensure_server(app: &AppHandle) -> BridgeResult<String> {
    let state = app.state::<ConnectorMcpState>();
    let mut runtime = state.runtime.lock().await;
    if let Some(server) = runtime.as_ref().filter(|s| s.running()) {
        return Ok(server.endpoint.clone());
    }
    let server = server::start(app.clone()).await?;
    let endpoint = server.endpoint.clone();
    // Rebind persisted grants to this running application. Revocation metadata is
    // checked separately on every request, including after the remote read.
    for stored in load_grants(app)? {
        if policy::grant_active(
            &stored.grant.client_id,
            &stored.grant.client_id,
            stored.grant.expires_at,
            now(),
            stored.grant.revoked,
        ) {
            if let Ok(mut credential) = load_credential(&stored.grant.client_id) {
                credential.endpoint = endpoint.clone();
                credential.server_pid = std::process::id();
                save_credential(&stored.grant.client_id, &credential)?;
            }
        }
    }
    *runtime = Some(server);
    Ok(endpoint)
}

/// Restore an explicitly paired local service at app startup. No network or
/// browser access occurs here. With no live grants no listener is created.
pub async fn restore(app: &AppHandle) -> BridgeResult<()> {
    if load_grants(app)?
        .iter()
        .any(|g| !g.grant.revoked && g.grant.expires_at > now())
    {
        ensure_server(app).await?;
    }
    Ok(())
}

pub async fn status(app: &AppHandle) -> BridgeResult<McpStatus> {
    let state = app.state::<ConnectorMcpState>();
    let running = state
        .runtime
        .lock()
        .await
        .as_ref()
        .is_some_and(|s| s.running());
    Ok(McpStatus {
        running,
        grants: load_grants(app)?.into_iter().map(|g| g.grant).collect(),
    })
}

pub async fn pair(
    app: &AppHandle,
    label: String,
    mut connection_ids: Vec<String>,
) -> BridgeResult<McpGrant> {
    if label.trim().is_empty()
        || label.len() > 80
        || label.chars().any(char::is_control)
        || connection_ids.is_empty()
        || connection_ids.len() > MAX_CONNECTIONS
    {
        return Err("invalid_pairing".into());
    }
    connection_ids.sort();
    connection_ids.dedup();
    let state = app.state::<ConnectorMcpState>();
    let _guard = state.mutation.lock().await;
    let mut grants = load_grants(app)?;
    grants.retain(|g| !g.grant.revoked && g.grant.expires_at > now());
    if grants.len() >= MAX_GRANTS {
        return Err("grant_limit".into());
    }
    let mut revisions = Vec::new();
    for id in &connection_ids {
        let config = connectors::get(app, id).map_err(|_| "invalid_connection")?;
        if !config.enabled {
            return Err("connection_disabled".into());
        }
        revisions.push(config.policy_revision);
    }
    let endpoint = ensure_server(app).await?;
    let grant = McpGrant {
        client_id: uuid::Uuid::new_v4().to_string(),
        label: label.trim().to_owned(),
        connection_ids,
        expires_at: now().saturating_add(GRANT_TTL),
        revoked: false,
    };
    let credential = PairCredential {
        token: format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        ),
        endpoint,
        server_pid: std::process::id(),
    };
    save_credential(&grant.client_id, &credential)?;
    grants.push(StoredGrant {
        grant: grant.clone(),
        revisions,
    });
    if let Err(error) = save_grants(app, &grants) {
        let _ = secrets::secret_store().delete(&key(&grant.client_id)?);
        return Err(error);
    }
    Ok(grant)
}

pub async fn revoke(app: &AppHandle, id: &str) -> BridgeResult<()> {
    let vault_key = key(id)?;
    let state = app.state::<ConnectorMcpState>();
    let _guard = state.mutation.lock().await;
    let mut grants = load_grants(app)?;
    let grant = grants
        .iter_mut()
        .find(|g| g.grant.client_id == id)
        .ok_or("pairing_required")?;
    grant.grant.revoked = true;
    // Durable revocation precedes vault cleanup: failures cannot restore access.
    save_grants(app, &grants)?;
    secrets::secret_store()
        .delete(&vault_key)
        .map_err(|_| "vault_unavailable".to_owned())
}

pub async fn configuration(app: &AppHandle, id: &str) -> BridgeResult<McpConfiguration> {
    active_grant(app, id)?;
    ensure_server(app).await?;
    let executable = std::env::current_exe().map_err(|_| "executable_unavailable")?;
    let command = executable
        .to_str()
        .ok_or("executable_unavailable")?
        .to_owned();
    Ok(McpConfiguration {
        command,
        args: vec![
            "--connectors-mcp".into(),
            "--mcp-client-id".into(),
            id.to_owned(),
        ],
        transport: "stdio".into(),
    })
}

fn active_grant(app: &AppHandle, id: &str) -> BridgeResult<StoredGrant> {
    key(id)?;
    if crate::settings::get_settings(app).offline_mode {
        return Err("offline".into());
    }
    load_grants(app)?
        .into_iter()
        .find(|g| {
            policy::grant_active(
                &g.grant.client_id,
                id,
                g.grant.expires_at,
                now(),
                g.grant.revoked,
            )
        })
        .ok_or("pairing_required".into())
}

fn allowed_configs(
    app: &AppHandle,
    grant: &StoredGrant,
) -> BridgeResult<Vec<connectors::types::ConnectorConfig>> {
    let configs = connectors::list(app).map_err(|_| "connections_unavailable")?;
    let offline = crate::settings::get_settings(app).offline_mode;
    Ok(configs
        .into_iter()
        .filter(|c| {
            policy::connection_allowed(&grant.grant.connection_ids, &c.id, c.enabled, offline)
                && grant
                    .grant
                    .connection_ids
                    .iter()
                    .position(|id| id == &c.id)
                    .is_some_and(|index| grant.revisions[index] == c.policy_revision)
        })
        .collect())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct IpcRequest {
    version: u8,
    client_id: String,
    token: String,
    call: BridgeCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum BridgeCall {
    Tools,
    List,
    Read {
        connection_id: String,
        kind: ConnectorKind,
        resource_id: String,
        project_id: Option<String>,
    },
    Query {
        connection_id: String,
        kind: ConnectorKind,
        project_id: Option<String>,
    },
}

pub(super) async fn dispatch(app: &AppHandle, request: IpcRequest) -> BridgeResult<Value> {
    if request.version != 1 {
        return Err("unsupported_version".into());
    }
    let grant = active_grant(app, &request.client_id)?;
    let secret = load_credential(&request.client_id)?;
    if secret.server_pid != std::process::id()
        || !policy::secret_matches(&secret.token, &request.token)
    {
        return Err("pairing_required".into());
    }
    let configs = allowed_configs(app, &grant)?;
    let result = match request.call {
        BridgeCall::Tools => serde_json::to_value(stdio::tools_for(&configs))
            .map_err(|_| "invalid_response".to_owned())?,
        BridgeCall::List => {
            serde_json::to_value(&configs).map_err(|_| "invalid_response".to_owned())?
        }
        BridgeCall::Read {
            connection_id,
            kind,
            resource_id,
            project_id,
        } => {
            if resource_id.is_empty() || resource_id.len() > 128 {
                return Err("invalid_resource".into());
            }
            require_connection(&configs, &connection_id, kind)?;
            let doc = connectors::read(app, &connection_id, &resource_id, project_id.as_deref())
                .await
                .map_err(|e| e.to_string())?;
            serde_json::to_value(doc).map_err(|_| "invalid_response".to_owned())?
        }
        BridgeCall::Query {
            connection_id,
            kind,
            project_id,
        } => {
            require_connection(&configs, &connection_id, kind)?;
            let docs = connectors::query(app, &connection_id, project_id.as_deref())
                .await
                .map_err(|e| e.to_string())?;
            serde_json::to_value(docs).map_err(|_| "invalid_response".to_owned())?
        }
    };
    // Do not disclose a result after the user disconnected, narrowed the scope,
    // revoked this grant, or enabled offline while the remote request ran.
    let final_grant = active_grant(app, &request.client_id)?;
    let final_configs = allowed_configs(app, &final_grant)?;
    if configs.iter().any(|c| {
        !final_configs
            .iter()
            .any(|current| current.id == c.id && current.policy_revision == c.policy_revision)
    }) {
        return Err("policy_changed".into());
    }
    Ok(result)
}

fn require_connection(
    configs: &[connectors::types::ConnectorConfig],
    id: &str,
    kind: ConnectorKind,
) -> BridgeResult<()> {
    if configs.iter().any(|c| c.id == id && c.kind == kind) {
        Ok(())
    } else {
        Err("scope_denied".into())
    }
}
