//! Public, secret-free connector contract. Authentication bundles stay in the vault.
use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorKind {
    Notion,
    AzureDevops,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorStatus {
    Disconnected,
    Authorizing,
    Ready,
    ReconnectRequired,
    TemporaryFailure,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum SprintPolicy {
    Ask,
    Fixed,
    CurrentTeam,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
pub struct ConnectorScope {
    pub notion_page_ids: Vec<String>,
    pub azure_project_ids: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AzureDestinationDefaults {
    pub project_id: String,
    pub team_id: String,
    pub backlog_id: Option<String>,
    pub area_path: Option<String>,
    pub work_item_type: Option<String>,
    pub sprint_policy: SprintPolicy,
    pub iteration_id: Option<String>,
    pub iteration_path: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ConnectorInput {
    pub kind: ConnectorKind,
    pub label: String,
    pub enabled: bool,
    pub organization: Option<String>,
    pub tenant: Option<String>,
    pub client_id: Option<String>,
    pub scope: ConnectorScope,
    pub default_notion_page_id: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ConnectorConfig {
    pub id: String,
    pub kind: ConnectorKind,
    pub label: String,
    pub enabled: bool,
    pub organization: Option<String>,
    pub tenant: Option<String>,
    pub client_id: Option<String>,
    pub scope: ConnectorScope,
    pub policy_revision: u32,
    pub status: ConnectorStatus,
    pub identity_hint: Option<String>,
    pub default_notion_page_id: Option<String>,
    pub azure_defaults: Option<AzureDestinationDefaults>,
    /// DCR redirect is public metadata; never contains an authorization code.
    pub oauth_redirect: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct CatalogEntry {
    pub id: String,
    pub name: String,
    pub path: Option<String>,
    pub start_date: Option<String>,
    pub finish_date: Option<String>,
    pub is_current: bool,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
pub struct AzureCatalog {
    pub projects: Vec<CatalogEntry>,
    pub teams: Vec<CatalogEntry>,
    pub backlogs: Vec<CatalogEntry>,
    pub areas: Vec<CatalogEntry>,
    pub iterations: Vec<CatalogEntry>,
    pub work_item_types: Vec<CatalogEntry>,
}
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ConnectionTest {
    pub identity_hint: Option<String>,
    pub readable: bool,
    pub publication: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct RemoteDocument {
    pub connection_id: String,
    pub id: String,
    pub url: String,
    pub title: String,
    pub text: String,
    pub revision: Option<u32>,
    pub truncated: bool,
}
/// Typed work-item draft reviewed by the user before any remote write
/// (FR-013-11). The backend composes the JSON Patch; no field is free-form
/// beyond title/description text.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct AzureWorkItemDraft {
    pub project_id: String,
    pub work_item_type: String,
    pub title: String,
    pub description: String,
    pub area_path: Option<String>,
    pub iteration_path: Option<String>,
    pub parent_id: Option<u32>,
}
/// One AI suggestion card. Proposal material only — it can never carry an
/// approval or become a remote write without going through prepare/execute.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ItemSuggestion {
    pub title: String,
    pub description: String,
    pub work_item_type: String,
    pub rationale: String,
    pub source_excerpt: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum OperationStatus {
    AwaitingApproval,
    Executing,
    Succeeded,
    Failed,
    OutcomeUnknown,
    Cancelled,
}
/// Journal row (FR-013-12): metadata + lifecycle only; the reviewed payload
/// lives in `payload_json` so reconcile is possible, never in logs.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct OperationRecord {
    pub id: String,
    pub connection_id: String,
    pub meeting_id: Option<String>,
    pub action: String,
    pub status: OperationStatus,
    pub remote_id: Option<String>,
    pub remote_url: Option<String>,
    pub error_code: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}
/// What the review dialog shows; `execute` binds to `fingerprint`, so any
/// post-review edit requires a new prepare (FR-013-11/26).
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct PreparedAction {
    pub operation_id: String,
    pub fingerprint: String,
    pub draft: AzureWorkItemDraft,
    pub warnings: Vec<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorErrorCode {
    AuthInvalid,
    PermissionDenied,
    ResourceUnavailable,
    PolicyDenied,
    Offline,
    RateLimited,
    Timeout,
    PayloadTooLarge,
    InvalidSchema,
    Cancelled,
    UnsupportedCapability,
    VaultUnavailable,
    TemporaryFailure,
    Busy,
}
#[derive(Debug, Clone, Serialize, Type)]
pub struct ConnectorError {
    pub code: ConnectorErrorCode,
    pub message: String,
}
impl ConnectorError {
    pub fn new(code: ConnectorErrorCode) -> Self {
        Self {
            code,
            message: format!("connector.{code:?}"),
        }
    }
}
impl std::fmt::Display for ConnectorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.code)
    }
}
impl std::error::Error for ConnectorError {}
pub type ConnectorResult<T> = Result<T, ConnectorError>;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn config_serialization_never_has_secret_fields() {
        let scope = ConnectorScope::default();
        let value = serde_json::to_value(scope).unwrap();
        assert!(value.get("access_token").is_none());
        assert_eq!(
            serde_json::to_string(&ConnectorKind::AzureDevops).unwrap(),
            "\"azure_devops\""
        );
    }
}
