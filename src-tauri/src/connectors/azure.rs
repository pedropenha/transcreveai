//! Azure DevOps REST reads: URLs and WIQL are composed only by this adapter.
use super::{http, policy, types::*};
use serde_json::{json, Value};
fn invalid() -> ConnectorError {
    ConnectorError::new(ConnectorErrorCode::InvalidSchema)
}
fn base(config: &ConnectorConfig) -> ConnectorResult<String> {
    Ok(format!(
        "https://dev.azure.com/{}",
        policy::segment(config.organization.as_deref().ok_or_else(invalid)?)?
    ))
}
async fn get(config: &ConnectorConfig, token: &str, path: &str) -> ConnectorResult<Value> {
    http::json(
        http::client()?
            .get(format!("{}{path}", base(config)?))
            .bearer_auth(token),
    )
    .await
}
pub async fn identity(config: &ConnectorConfig, token: &str) -> ConnectorResult<Option<String>> {
    let value = get(
        config,
        token,
        "/_apis/connectionData?connectOptions=1&lastChangeId=-1&lastChangeId64=-1",
    )
    .await?;
    Ok(value["authenticatedUser"]["providerDisplayName"]
        .as_str()
        .map(|s| s.chars().take(120).collect()))
}
fn entry(v: &Value) -> Option<CatalogEntry> {
    let name = v["name"].as_str()?;
    let id = v["id"]
        .as_str()
        .or_else(|| v["identifier"].as_str())
        .unwrap_or(name);
    Some(CatalogEntry {
        id: id.to_string(),
        name: name.chars().take(160).collect(),
        path: v["path"].as_str().map(String::from),
        start_date: v["attributes"]["startDate"].as_str().map(String::from),
        finish_date: v["attributes"]["finishDate"].as_str().map(String::from),
        is_current: v["attributes"]["timeFrame"].as_str() == Some("current"),
    })
}
fn entries(v: &Value) -> Vec<CatalogEntry> {
    v["value"]
        .as_array()
        .map(|a| a.iter().take(150).filter_map(entry).collect())
        .unwrap_or_default()
}
fn areas(v: &Value, result: &mut Vec<CatalogEntry>, depth: usize) {
    if depth > 8 || result.len() >= 150 {
        return;
    }
    if let Some(mut e) = entry(v) {
        e.path = e
            .path
            .map(|p| p.trim_start_matches('\\').replacen("\\Area\\", "\\", 1));
        result.push(e);
    }
    if let Some(children) = v["children"].as_array() {
        for child in children {
            areas(child, result, depth + 1);
        }
    }
}
pub async fn catalog(
    config: &ConnectorConfig,
    token: &str,
    project: Option<&str>,
    team: Option<&str>,
) -> ConnectorResult<AzureCatalog> {
    let mut c = AzureCatalog::default();
    if project.is_none() {
        c.projects =
            entries(&get(config, token, "/_apis/projects?api-version=7.1&$top=150").await?);
        return Ok(c);
    }
    let project = policy::require_project(config, project.ok_or_else(invalid)?)?;
    c.teams = entries(
        &get(
            config,
            token,
            &format!("/_apis/projects/{project}/teams?api-version=7.1&$top=150"),
        )
        .await?,
    );
    c.work_item_types = entries(
        &get(
            config,
            token,
            &format!("/{project}/_apis/wit/workitemtypes?api-version=7.1"),
        )
        .await?,
    );
    let area = get(
        config,
        token,
        &format!("/{project}/_apis/wit/classificationnodes/areas?$depth=8&api-version=7.1"),
    )
    .await?;
    areas(&area, &mut c.areas, 0);
    if let Some(team) = team {
        let team = policy::canonical_id(team)?;
        if !c.teams.iter().any(|e| e.id.eq_ignore_ascii_case(&team)) {
            return Err(ConnectorError::new(ConnectorErrorCode::PolicyDenied));
        }
        c.backlogs = entries(
            &get(
                config,
                token,
                &format!("/{project}/{team}/_apis/work/backlogs?api-version=7.1"),
            )
            .await?,
        );
        c.iterations = entries(
            &get(
                config,
                token,
                &format!("/{project}/{team}/_apis/work/teamsettings/iterations?api-version=7.1"),
            )
            .await?,
        );
        let current=entries(&get(config,token,&format!("/{project}/{team}/_apis/work/teamsettings/iterations?$timeframe=current&api-version=7.1")).await?);
        for item in &mut c.iterations {
            item.is_current = current.iter().any(|e| e.id == item.id);
        }
    }
    Ok(c)
}
pub async fn read(
    config: &ConnectorConfig,
    token: &str,
    id: &str,
    project: &str,
) -> ConnectorResult<RemoteDocument> {
    let project = policy::require_project(config, project)?;
    let id = id
        .parse::<u32>()
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(invalid)?;
    let v=get(config,token,&format!("/{project}/_apis/wit/workitems/{id}?fields=System.Id,System.Title,System.Description,System.TeamProject&api-version=7.1")).await?;
    // Project-scoped Azure API path enforces project ownership; additionally
    // validate the returned project ID via the canonical project lookup.
    let remote_project = v["fields"]["System.TeamProject"]
        .as_str()
        .ok_or_else(invalid)?;
    let p = get(
        config,
        token,
        &format!("/_apis/projects/{project}?api-version=7.1"),
    )
    .await?;
    if p["id"]
        .as_str()
        .is_none_or(|p| !p.eq_ignore_ascii_case(&project))
        || p["name"].as_str() != Some(remote_project)
    {
        return Err(ConnectorError::new(ConnectorErrorCode::PolicyDenied));
    }
    let text = v["fields"]["System.Description"].as_str().unwrap_or("");
    Ok(RemoteDocument {
        connection_id: config.id.clone(),
        id: id.to_string(),
        url: format!("{}/{project}/_workitems/edit/{id}", base(config)?),
        title: v["fields"]["System.Title"]
            .as_str()
            .unwrap_or("")
            .chars()
            .take(256)
            .collect(),
        text: text.chars().take(8192).collect(),
        revision: v["rev"].as_u64().and_then(|n| u32::try_from(n).ok()),
        truncated: text.chars().count() > 8192,
    })
}
pub async fn query(
    config: &ConnectorConfig,
    token: &str,
    project: &str,
) -> ConnectorResult<Vec<RemoteDocument>> {
    let project = policy::require_project(config, project)?;
    let v=http::json(http::client()?.post(format!("{}/{project}/_apis/wit/wiql?api-version=7.1&$top=10",base(config)?)).bearer_auth(token).json(&json!({"query":"SELECT [System.Id] FROM WorkItems WHERE [System.TeamProject] = @project ORDER BY [System.ChangedDate] DESC"}))).await?;
    let mut result = Vec::new();
    if let Some(items) = v["workItems"].as_array() {
        let mut bytes = 0;
        for item in items.iter().take(10) {
            let id = item["id"].as_u64().ok_or_else(invalid)?.to_string();
            let doc = read(config, token, &id, &project).await?;
            bytes += doc.text.len() + doc.title.len();
            if bytes > 32 * 1024 {
                break;
            }
            result.push(doc);
        }
    }
    Ok(result)
}
