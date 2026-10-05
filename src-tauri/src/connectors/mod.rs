//! Connector facade shared by UI and the explicitly paired local MCP bridge.
mod azure;
mod http;
mod journal;
mod notion;
mod oauth;
pub mod policy;
pub mod types;
pub mod vault;
use sha2::Digest;
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

// --- Etapa 2: propostas, aprovação e execução (somente UI confiável) --------

fn db(app: &AppHandle) -> ConnectorResult<rusqlite::Connection> {
    crate::meeting::session::open_session_db(app)
        .map_err(|_| ConnectorError::new(ConnectorErrorCode::TemporaryFailure))
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

fn clean_text(value: &str, max: usize, required: bool) -> ConnectorResult<String> {
    let trimmed = value.trim();
    let bad_control = |c: char| c.is_control() && !matches!(c, '\n' | '\r' | '\t');
    if trimmed.is_empty() && required
        || trimmed.chars().count() > max
        || trimmed.chars().any(bad_control)
    {
        return Err(ConnectorError::new(ConnectorErrorCode::InvalidSchema));
    }
    Ok(trimmed.to_string())
}

fn meeting_id_checked(id: &str) -> ConnectorResult<String> {
    let id = id.trim();
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err(ConnectorError::new(ConnectorErrorCode::InvalidSchema));
    }
    Ok(id.to_string())
}

/// Validates a reviewed draft against the real catalog and persists it as an
/// `awaiting_approval` operation. Nothing reaches Azure here (FR-013-11).
pub async fn prepare_azure_item(
    app: &AppHandle,
    connection_id: &str,
    meeting_id: Option<String>,
    mut draft: AzureWorkItemDraft,
) -> ConnectorResult<PreparedAction> {
    online(app)?;
    let id = policy::canonical_id(connection_id)?;
    let lock = lock_for(&id)?;
    let _guard = lock.lock().await;
    let c = get(app, &id)?;
    if c.kind != ConnectorKind::AzureDevops {
        return Err(ConnectorError::new(
            ConnectorErrorCode::UnsupportedCapability,
        ));
    }
    let meeting_id = meeting_id.as_deref().map(meeting_id_checked).transpose()?;
    draft.project_id = policy::require_project(&c, &draft.project_id)?;
    draft.title = clean_text(&draft.title, 256, true)?;
    draft.description = clean_text(&draft.description, 8192, false)?;
    draft.area_path = draft
        .area_path
        .as_deref()
        .map(|v| clean_text(v, 256, false))
        .transpose()?;
    draft.iteration_path = draft
        .iteration_path
        .as_deref()
        .map(|v| clean_text(v, 256, false))
        .transpose()?;
    let token = token(app, &c).await?;
    let catalog = azure::catalog(&c, &token, Some(&draft.project_id), None).await?;
    let mut warnings = Vec::new();
    let work_type = catalog
        .work_item_types
        .iter()
        .find(|t| t.name.eq_ignore_ascii_case(&draft.work_item_type))
        .map(|t| t.name.clone())
        .ok_or_else(|| ConnectorError::new(ConnectorErrorCode::InvalidSchema))?;
    draft.work_item_type = work_type;
    if let Some(area) = &draft.area_path {
        if !catalog
            .areas
            .iter()
            .any(|a| a.path.as_deref() == Some(area))
        {
            return Err(ConnectorError::new(ConnectorErrorCode::InvalidSchema));
        }
    }
    if draft.iteration_path.is_some() {
        warnings.push("iteration_unverified".to_string());
    }
    if let Some(parent) = draft.parent_id {
        azure::read(&c, &token, &parent.to_string(), &draft.project_id).await?;
    }
    let operation_id = uuid::Uuid::new_v4().to_string();
    let canonical = serde_json::to_vec(&(&c.id, c.policy_revision, meeting_id.as_deref(), &draft))
        .map_err(|_| ConnectorError::new(ConnectorErrorCode::InvalidSchema))?;
    let fingerprint = format!("{:x}", sha2::Sha256::digest(&canonical));
    let created = now();
    let record = OperationRecord {
        id: operation_id.clone(),
        connection_id: c.id.clone(),
        meeting_id,
        action: "azure_create_work_item".to_string(),
        status: OperationStatus::AwaitingApproval,
        remote_id: None,
        remote_url: None,
        error_code: None,
        created_at: created,
        updated_at: created,
    };
    journal::insert(&db(app)?, &record, &draft, &fingerprint)?;
    Ok(PreparedAction {
        operation_id,
        fingerprint,
        draft,
        warnings,
    })
}

/// Executes an approved operation. The fingerprint must match what the user
/// reviewed; already-claimed or finished operations return their persisted
/// state instead of running again (no blind retry, FR-013-12).
pub async fn execute_action(
    app: &AppHandle,
    operation_id: &str,
    fingerprint: &str,
) -> ConnectorResult<OperationRecord> {
    online(app)?;
    let operation_id = policy::canonical_id(operation_id)?;
    let conn = db(app)?;
    let Some((record, expected, draft)) = journal::get(&conn, &operation_id)? else {
        return Err(ConnectorError::new(ConnectorErrorCode::ResourceUnavailable));
    };
    if fingerprint != expected {
        return Err(ConnectorError::new(ConnectorErrorCode::PolicyDenied));
    }
    if record.status != OperationStatus::AwaitingApproval {
        return Ok(record);
    }
    let c = get(app, &record.connection_id)?;
    let lock = lock_for(&c.id)?;
    let _guard = lock.lock().await;
    if !journal::claim(&conn, &record.id, now())? {
        return journal::get(&conn, &operation_id)?
            .map(|(r, _, _)| r)
            .ok_or_else(|| ConnectorError::new(ConnectorErrorCode::ResourceUnavailable));
    }
    let outcome = async {
        policy::require_project(&c, &draft.project_id)?;
        let token = token(app, &c).await?;
        azure::create_work_item(&c, &token, &draft).await
    };
    let result = tokio::time::timeout(Duration::from_secs(30), outcome).await;
    let (status, remote_id, remote_url, error_code) = match result {
        Ok(Ok((id, url))) => (
            OperationStatus::Succeeded,
            Some(id.to_string()),
            Some(url),
            None,
        ),
        Ok(Err(e)) => (
            match e.code {
                ConnectorErrorCode::Timeout | ConnectorErrorCode::TemporaryFailure => {
                    OperationStatus::OutcomeUnknown
                }
                _ => OperationStatus::Failed,
            },
            None,
            None,
            Some(format!("{:?}", e.code)),
        ),
        Err(_) => (
            OperationStatus::OutcomeUnknown,
            None,
            None,
            Some("timeout".into()),
        ),
    };
    journal::finish(
        &conn,
        &record.id,
        status,
        remote_id.as_deref(),
        remote_url.as_deref(),
        error_code.as_deref(),
        now(),
    )?;
    if status == OperationStatus::Succeeded {
        if let (Some(meeting_id), Some(remote_id), Some(remote_url)) = (
            record.meeting_id.as_deref(),
            remote_id.as_deref(),
            remote_url.as_deref(),
        ) {
            journal::insert_link(
                &conn,
                meeting_id,
                &c.id,
                "azure_devops",
                remote_id,
                remote_url,
                &record.id,
                now(),
            )?;
        }
    }
    journal::get(&conn, &operation_id)?
        .map(|(r, _, _)| r)
        .ok_or_else(|| ConnectorError::new(ConnectorErrorCode::ResourceUnavailable))
}

/// Operations linked to a meeting — for persistence across reopen.
pub fn list_meeting_operations(
    app: &AppHandle,
    meeting_id: &str,
) -> ConnectorResult<Vec<OperationRecord>> {
    let meeting_id = meeting_id_checked(meeting_id)?;
    journal::list_by_meeting(&db(app)?, &meeting_id)
}

/// AI suggestion cards from a meeting summary (FR-013-28). The model never
/// executes anything: output is parsed, clamped and validated against the
/// real catalog; invalid cards are dropped, not guessed.
pub async fn suggest_azure_items(
    app: &AppHandle,
    connection_id: &str,
    meeting_id: &str,
) -> ConnectorResult<Vec<ItemSuggestion>> {
    online(app)?;
    let id = policy::canonical_id(connection_id)?;
    let meeting_id = meeting_id_checked(meeting_id)?;
    let lock = lock_for(&id)?;
    let _guard = lock.lock().await;
    let c = get(app, &id)?;
    if c.kind != ConnectorKind::AzureDevops {
        return Err(ConnectorError::new(
            ConnectorErrorCode::UnsupportedCapability,
        ));
    }
    let conn = db(app)?;
    use crate::db::meetings::MeetingRepository;
    let meeting = crate::db::meetings::SqliteMeetingRepository::new(&conn)
        .get(&meeting_id)
        .map_err(|_| ConnectorError::new(ConnectorErrorCode::TemporaryFailure))?
        .ok_or_else(|| ConnectorError::new(ConnectorErrorCode::ResourceUnavailable))?;
    let summary = meeting
        .summary_md
        .filter(|s| meeting.summary_status == "ready" && !s.trim().is_empty())
        .ok_or_else(|| ConnectorError::new(ConnectorErrorCode::ResourceUnavailable))?;
    let token = token(app, &c).await?;
    let catalog = azure::catalog(&c, &token, None, None).await?;
    let types: Vec<String> = catalog
        .work_item_types
        .iter()
        .map(|t| t.name.clone())
        .collect();
    // project-less catalog returns no types; fetch per allowed project
    let types = if types.is_empty() {
        let mut all = Vec::new();
        for project in &c.scope.azure_project_ids {
            let scoped = azure::catalog(&c, &token, Some(project), None).await?;
            all.extend(scoped.work_item_types.iter().map(|t| t.name.clone()));
        }
        all
    } else {
        types
    };
    if types.is_empty() {
        return Err(ConnectorError::new(ConnectorErrorCode::ResourceUnavailable));
    }
    let settings = crate::settings::get_settings(app);
    let provider = settings
        .post_process_provider(&settings.post_process_provider_id)
        .ok_or_else(|| ConnectorError::new(ConnectorErrorCode::UnsupportedCapability))?;
    let api_key = crate::secrets::provider_api_key(app, &provider.id).unwrap_or_default();
    if crate::llm::router::requires_api_key(&provider) && api_key.trim().is_empty() {
        return Err(ConnectorError::new(
            ConnectorErrorCode::UnsupportedCapability,
        ));
    }
    let model = settings
        .post_process_models
        .get(&provider.id)
        .cloned()
        .unwrap_or_default();
    let llm = crate::llm::router::build_provider(&crate::llm::router::LlmRoute {
        provider: provider.clone(),
        model,
        api_key,
        escalated: false,
        cli_agent: crate::llm::cli_agent::is_cli_agent(&provider.id)
            .then(|| settings.cli_agent_config(&provider.id)),
    })
    .map_err(|_| ConnectorError::new(ConnectorErrorCode::UnsupportedCapability))?;
    let excerpt: String = summary.chars().take(12_000).collect();
    let prompt = format!(
        "Return ONLY a JSON array (max 6 items) of work items this meeting \
         summary suggests for Azure DevOps. Each item: \
         {{\"title\": string, \"description\": string, \"work_item_type\": string, \
         \"rationale\": string, \"source_excerpt\": string}}. \
         work_item_type must be one of: {}. source_excerpt must be a verbatim \
         quote from the summary. No markdown fences.",
        types.join(", ")
    );
    let response = llm
        .complete(crate::llm::types::LlmRequest {
            system: "You extract actionable work items from meeting summaries. \
                     Output strict JSON only."
                .to_string(),
            messages: vec![crate::llm::types::LlmMessage {
                role: crate::llm::types::LlmRole::User,
                content: format!("{prompt}\n\nMEETING SUMMARY:\n{excerpt}"),
            }],
            max_tokens: 2048,
            temperature: 0.2,
            timeout: Duration::from_secs(60),
            purpose: crate::llm::types::LlmPurpose::Summary,
        })
        .await
        .map_err(|_| ConnectorError::new(ConnectorErrorCode::TemporaryFailure))?;
    let text = response.text;
    let start = text.find('[').unwrap_or(0);
    let end = text.rfind(']').map(|i| i + 1).unwrap_or(text.len());
    let parsed: Vec<serde_json::Value> = if start < end {
        serde_json::from_str(&text[start..end]).unwrap_or_default()
    } else {
        Vec::new()
    };
    let mut suggestions = Vec::new();
    for value in parsed.into_iter().take(6) {
        let title = value["title"].as_str().unwrap_or_default().trim();
        if title.is_empty() {
            continue;
        }
        let suggested = value["work_item_type"].as_str().unwrap_or_default();
        let work_item_type = types
            .iter()
            .find(|t| t.eq_ignore_ascii_case(suggested))
            .cloned()
            .or_else(|| {
                types
                    .iter()
                    .find(|t| t.eq_ignore_ascii_case("task"))
                    .cloned()
                    .or_else(|| types.first().cloned())
            })
            .unwrap_or_default();
        suggestions.push(ItemSuggestion {
            title: title.chars().take(200).collect(),
            description: value["description"]
                .as_str()
                .unwrap_or_default()
                .chars()
                .take(4_000)
                .collect(),
            work_item_type,
            rationale: value["rationale"]
                .as_str()
                .unwrap_or_default()
                .chars()
                .take(400)
                .collect(),
            source_excerpt: value["source_excerpt"]
                .as_str()
                .unwrap_or_default()
                .chars()
                .take(400)
                .collect(),
        });
    }
    Ok(suggestions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_text_trims_and_enforces_bounds() {
        assert_eq!(clean_text("  hello  ", 256, true).unwrap(), "hello");
        assert!(clean_text("   ", 256, true).is_err());
        assert_eq!(clean_text("   ", 256, false).unwrap(), "");
        assert!(clean_text("abc", 2, true).is_err());
    }

    #[test]
    fn clean_text_allows_markdown_but_rejects_other_controls() {
        let markdown = "## Plano\n- item 1\n\t- sub";
        assert_eq!(clean_text(markdown, 256, true).unwrap(), markdown);
        assert!(clean_text("bad\u{0}char", 256, true).is_err());
        assert!(clean_text("bad\u{7}char", 256, true).is_err());
    }

    #[test]
    fn meeting_id_checked_accepts_ids_and_rejects_injection() {
        assert_eq!(meeting_id_checked("mtg_01-ABC").unwrap(), "mtg_01-ABC");
        assert!(meeting_id_checked("").is_err());
        assert!(meeting_id_checked("a; drop table").is_err());
        assert!(meeting_id_checked("../etc").is_err());
        assert!(meeting_id_checked(&"x".repeat(65)).is_err());
    }
}
