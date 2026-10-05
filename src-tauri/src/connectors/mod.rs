//! Connector facade shared by UI and the explicitly paired local MCP bridge.
mod azure;
mod http;
mod notion;
mod oauth;
pub mod policy;
pub mod types;
pub mod vault;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;
use tauri_plugin_store::StoreExt;
use types::*;
const STORE: &str = "connector_store.json";
static STORE_LOCK: Mutex<()> = Mutex::new(());
type GrantLocks = Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>;
static LOCKS: OnceLock<GrantLocks> = OnceLock::new();
static LOGINS: OnceLock<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>> = OnceLock::new();
fn lock_for(id: &str) -> ConnectorResult<Arc<tokio::sync::Mutex<()>>> {
    let mut map = LOCKS
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| ConnectorError::new(ConnectorErrorCode::TemporaryFailure))?;
    Ok(map
        .entry(id.to_string())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone())
}
pub fn online(app: &AppHandle) -> ConnectorResult<()> {
    if crate::settings::get_settings(app).offline_mode {
        Err(ConnectorError::new(ConnectorErrorCode::Offline))
    } else {
        Ok(())
    }
}
pub fn list(app: &AppHandle) -> ConnectorResult<Vec<ConnectorConfig>> {
    let store = app
        .store(crate::portable::store_path(STORE))
        .map_err(|_| ConnectorError::new(ConnectorErrorCode::TemporaryFailure))?;
    let value = store
        .get("connections")
        .unwrap_or_else(|| serde_json::json!([]));
    serde_json::from_value(value)
        .map_err(|_| ConnectorError::new(ConnectorErrorCode::InvalidSchema))
}
pub fn get(app: &AppHandle, id: &str) -> ConnectorResult<ConnectorConfig> {
    let id = policy::canonical_id(id)?;
    list(app)?
        .into_iter()
        .find(|c| c.id == id)
        .ok_or_else(|| ConnectorError::new(ConnectorErrorCode::ResourceUnavailable))
}
fn put(app: &AppHandle, config: ConnectorConfig) -> ConnectorResult<ConnectorConfig> {
    let _guard = STORE_LOCK
        .lock()
        .map_err(|_| ConnectorError::new(ConnectorErrorCode::TemporaryFailure))?;
    let mut all = list(app)?;
    if let Some(old) = all.iter_mut().find(|c| c.id == config.id) {
        *old = config.clone();
    } else {
        if all.len() >= 20 {
            return Err(ConnectorError::new(ConnectorErrorCode::InvalidSchema));
        }
        all.push(config.clone());
    }
    persist(app, &all)?;
    Ok(config)
}
fn persist(app: &AppHandle, all: &[ConnectorConfig]) -> ConnectorResult<()> {
    let store = app
        .store(crate::portable::store_path(STORE))
        .map_err(|_| ConnectorError::new(ConnectorErrorCode::TemporaryFailure))?;
    let previous = store.get("connections");
    let previous_schema = store.get("schema_version");
    store.set("schema_version", 1);
    store.set(
        "connections",
        serde_json::to_value(all)
            .map_err(|_| ConnectorError::new(ConnectorErrorCode::InvalidSchema))?,
    );
    if store.save().is_err() {
        if let Some(value) = previous {
            store.set("connections", value);
        } else {
            store.delete("connections");
        }
        if let Some(value) = previous_schema {
            store.set("schema_version", value);
        } else {
            store.delete("schema_version");
        }
        return Err(ConnectorError::new(ConnectorErrorCode::TemporaryFailure));
    }
    Ok(())
}
pub async fn save(
    app: &AppHandle,
    mut input: ConnectorInput,
    id: Option<&str>,
) -> ConnectorResult<ConnectorConfig> {
    policy::validate_input(&mut input)?;
    let id = id
        .map(policy::canonical_id)
        .transpose()?
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let lock = lock_for(&id)?;
    let _guard = lock.lock().await;
    let old = list(app)?.into_iter().find(|c| c.id == id);
    if id.is_empty() {
        return Err(ConnectorError::new(ConnectorErrorCode::InvalidSchema));
    }
    let auth_changed = old.as_ref().is_some_and(|c| {
        c.kind != input.kind
            || c.organization != input.organization
            || c.tenant != input.tenant
            || (c.kind == ConnectorKind::AzureDevops && c.client_id != input.client_id)
    });
    if auth_changed {
        cancel_login(&id)?;
        vault::delete(crate::secrets::secret_store().as_ref(), &id)?;
    }
    let mut status = old
        .as_ref()
        .filter(|_| !auth_changed)
        .map(|c| c.status)
        .unwrap_or(ConnectorStatus::Disconnected);
    if status == ConnectorStatus::Authorizing {
        cancel_login(&id)?;
        status = ConnectorStatus::Disconnected;
    }
    put(
        app,
        ConnectorConfig {
            id,
            kind: input.kind,
            label: input.label.trim().to_string(),
            enabled: input.enabled,
            organization: input.organization,
            tenant: input.tenant,
            client_id: if input.kind == ConnectorKind::Notion {
                old.as_ref()
                    .filter(|_| !auth_changed)
                    .and_then(|c| c.client_id.clone())
            } else {
                input.client_id
            },
            scope: input.scope,
            policy_revision: old.as_ref().map(|c| c.policy_revision + 1).unwrap_or(1),
            status,
            identity_hint: old
                .as_ref()
                .filter(|_| !auth_changed)
                .and_then(|c| c.identity_hint.clone()),
            default_notion_page_id: input.default_notion_page_id,
            azure_defaults: None,
            oauth_redirect: old
                .as_ref()
                .filter(|_| !auth_changed)
                .and_then(|c| c.oauth_redirect.clone()),
        },
    )
}
fn cancel_login(id: &str) -> ConnectorResult<()> {
    if let Some(task) = LOGINS
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| ConnectorError::new(ConnectorErrorCode::TemporaryFailure))?
        .remove(id)
    {
        task.abort();
    }
    Ok(())
}
pub async fn cancel_oauth(app: &AppHandle, id: &str) -> ConnectorResult<()> {
    let id = policy::canonical_id(id)?;
    cancel_login(&id)?;
    let lock = lock_for(&id)?;
    let _guard = lock.lock().await;
    let mut c = get(app, &id)?;
    if c.status == ConnectorStatus::Authorizing {
        c.status = ConnectorStatus::Disconnected;
        put(app, c)?;
    }
    Ok(())
}
pub async fn disconnect(app: &AppHandle, id: &str) -> ConnectorResult<()> {
    let id = policy::canonical_id(id)?;
    cancel_login(&id)?;
    let lock = lock_for(&id)?;
    let _guard = lock.lock().await;
    let mut c = get(app, &id)?;
    vault::delete(crate::secrets::secret_store().as_ref(), &id)?;
    c.status = ConnectorStatus::Disconnected;
    c.identity_hint = None;
    c.policy_revision += 1;
    put(app, c)?;
    Ok(())
}
pub async fn delete(app: &AppHandle, id: &str) -> ConnectorResult<()> {
    let id = policy::canonical_id(id)?;
    cancel_login(&id)?;
    let lock = lock_for(&id)?;
    let _connection = lock.lock().await;
    get(app, &id)?;
    vault::delete(crate::secrets::secret_store().as_ref(), &id)?;
    let _guard = STORE_LOCK
        .lock()
        .map_err(|_| ConnectorError::new(ConnectorErrorCode::TemporaryFailure))?;
    let all = list(app)?
        .into_iter()
        .filter(|c| c.id != id)
        .collect::<Vec<_>>();
    persist(app, &all)
}
pub async fn begin_oauth(app: &AppHandle, id: &str) -> ConnectorResult<ConnectorConfig> {
    online(app)?;
    let id = policy::canonical_id(id)?;
    let lock = lock_for(&id)?;
    let _guard = lock.lock().await;
    let mut c = get(app, &id)?;
    if !c.enabled {
        return Err(ConnectorError::new(ConnectorErrorCode::PolicyDenied));
    }
    if c.status == ConnectorStatus::Authorizing {
        return Err(ConnectorError::new(ConnectorErrorCode::Busy));
    }
    let login = oauth::prepare(&mut c).await?;
    c.status = ConnectorStatus::Authorizing;
    let c = put(app, c)?;
    if app
        .opener()
        .open_url(&login.authorization_url, None::<&str>)
        .is_err()
    {
        let mut failed = c;
        failed.status = ConnectorStatus::TemporaryFailure;
        put(app, failed)?;
        return Err(ConnectorError::new(ConnectorErrorCode::TemporaryFailure));
    }
    let handle = app.clone();
    let connection = id.clone();
    let generation = c.policy_revision;
    let task = tokio::spawn(async move {
        let result = oauth::finish(login).await;
        let Ok(lock) = lock_for(&connection) else {
            return;
        };
        let _guard = lock.lock().await;
        let Ok(mut c) = get(&handle, &connection) else {
            return;
        };
        if c.policy_revision != generation
            || c.status != ConnectorStatus::Authorizing
            || online(&handle).is_err()
        {
            return;
        }
        match result {
            Ok(bundle) => match vault::save(
                crate::secrets::secret_store().as_ref(),
                &connection,
                &bundle,
            ) {
                Ok(()) => c.status = ConnectorStatus::Ready,
                Err(_) => c.status = ConnectorStatus::TemporaryFailure,
            },
            Err(e) => {
                c.status = if e.code == ConnectorErrorCode::AuthInvalid {
                    ConnectorStatus::ReconnectRequired
                } else {
                    ConnectorStatus::TemporaryFailure
                }
            }
        }
        let _ = put(&handle, c);
    });
    LOGINS
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| ConnectorError::new(ConnectorErrorCode::TemporaryFailure))?
        .insert(id, task);
    Ok(c)
}
async fn token(app: &AppHandle, c: &ConnectorConfig) -> ConnectorResult<String> {
    online(app)?;
    if !c.enabled || c.status != ConnectorStatus::Ready {
        return Err(ConnectorError::new(ConnectorErrorCode::AuthInvalid));
    }
    let store = crate::secrets::secret_store();
    let mut b = vault::load(store.as_ref(), &c.id)?
        .ok_or_else(|| ConnectorError::new(ConnectorErrorCode::AuthInvalid))?;
    if b.expires_at <= oauth::now() + 60 {
        if let Err(e) = oauth::refresh(&mut b).await {
            if e.code == ConnectorErrorCode::AuthInvalid {
                let mut failed = c.clone();
                failed.status = ConnectorStatus::ReconnectRequired;
                put(app, failed)?;
            }
            return Err(e);
        }
        vault::save(store.as_ref(), &c.id, &b)?;
    }
    Ok(b.access_token)
}
pub async fn catalog(
    app: &AppHandle,
    id: &str,
    project: Option<&str>,
    team: Option<&str>,
) -> ConnectorResult<AzureCatalog> {
    online(app)?;
    let id = policy::canonical_id(id)?;
    let lock = lock_for(&id)?;
    let _guard = lock.lock().await;
    let c = get(app, &id)?;
    if c.kind != ConnectorKind::AzureDevops {
        return Err(ConnectorError::new(
            ConnectorErrorCode::UnsupportedCapability,
        ));
    }
    let token = token(app, &c).await?;
    azure::catalog(&c, &token, project, team).await
}
pub async fn save_defaults(
    app: &AppHandle,
    id: &str,
    defaults: AzureDestinationDefaults,
) -> ConnectorResult<ConnectorConfig> {
    online(app)?;
    let id = policy::canonical_id(id)?;
    let lock = lock_for(&id)?;
    let _guard = lock.lock().await;
    let mut c = get(app, &id)?;
    policy::require_project(&c, &defaults.project_id)?;
    let token = token(app, &c).await?;
    let catalog = azure::catalog(
        &c,
        &token,
        Some(&defaults.project_id),
        Some(&defaults.team_id),
    )
    .await?;
    policy::validate_defaults(&defaults, &catalog)?;
    c.azure_defaults = Some(defaults);
    c.policy_revision += 1;
    put(app, c)
}
pub async fn read(
    app: &AppHandle,
    id: &str,
    resource: &str,
    project: Option<&str>,
) -> ConnectorResult<RemoteDocument> {
    online(app)?;
    let id = policy::canonical_id(id)?;
    let lock = lock_for(&id)?;
    let _guard = lock.lock().await;
    let c = get(app, &id)?;
    let token = token(app, &c).await?;
    let work = async {
        match c.kind {
            ConnectorKind::Notion => notion::read(&c, &token, resource).await,
            ConnectorKind::AzureDevops => {
                azure::read(
                    &c,
                    &token,
                    resource,
                    project
                        .ok_or_else(|| ConnectorError::new(ConnectorErrorCode::InvalidSchema))?,
                )
                .await
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(30), work)
        .await
        .map_err(|_| ConnectorError::new(ConnectorErrorCode::Timeout))?
}
pub async fn query(
    app: &AppHandle,
    id: &str,
    project: Option<&str>,
) -> ConnectorResult<Vec<RemoteDocument>> {
    online(app)?;
    let id = policy::canonical_id(id)?;
    let lock = lock_for(&id)?;
    let _guard = lock.lock().await;
    let c = get(app, &id)?;
    let token = token(app, &c).await?;
    if c.kind == ConnectorKind::Notion {
        return Err(ConnectorError::new(
            ConnectorErrorCode::UnsupportedCapability,
        ));
    }
    tokio::time::timeout(
        Duration::from_secs(30),
        azure::query(
            &c,
            &token,
            project.ok_or_else(|| ConnectorError::new(ConnectorErrorCode::InvalidSchema))?,
        ),
    )
    .await
    .map_err(|_| ConnectorError::new(ConnectorErrorCode::Timeout))?
}
pub async fn test(app: &AppHandle, id: &str) -> ConnectorResult<ConnectionTest> {
    online(app)?;
    let id = policy::canonical_id(id)?;
    let lock = lock_for(&id)?;
    let _guard = lock.lock().await;
    let mut c = get(app, &id)?;
    let token = token(app, &c).await?;
    let identity = match c.kind {
        ConnectorKind::Notion => {
            let page = c
                .default_notion_page_id
                .as_deref()
                .or_else(|| c.scope.notion_page_ids.first().map(String::as_str))
                .ok_or_else(|| ConnectorError::new(ConnectorErrorCode::PolicyDenied))?;
            tokio::time::timeout(Duration::from_secs(30), notion::read(&c, &token, page))
                .await
                .map_err(|_| ConnectorError::new(ConnectorErrorCode::Timeout))??;
            None
        }
        ConnectorKind::AzureDevops => {
            azure::catalog(&c, &token, None, None).await?;
            azure::identity(&c, &token).await.ok().flatten()
        }
    };
    if identity.is_some() {
        c.identity_hint = identity.clone();
        put(app, c)?;
    }
    Ok(ConnectionTest {
        identity_hint: identity,
        readable: true,
        publication: "unknown".into(),
    })
}
/// Restore interrupted login state without starting browser/network activity.
pub fn restore(app: &AppHandle) -> ConnectorResult<()> {
    for mut c in list(app)? {
        if c.status == ConnectorStatus::Authorizing {
            c.status = ConnectorStatus::Disconnected;
            put(app, c)?;
        }
    }
    Ok(())
}
