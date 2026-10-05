use super::types::*;

pub fn canonical_id(id: &str) -> ConnectorResult<String> {
    uuid::Uuid::parse_str(id)
        .map(|v| v.to_string())
        .map_err(|_| ConnectorError::new(ConnectorErrorCode::InvalidSchema))
}
pub fn segment(value: &str) -> ConnectorResult<&str> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        || value == "."
        || value == ".."
    {
        Err(ConnectorError::new(ConnectorErrorCode::InvalidSchema))
    } else {
        Ok(value)
    }
}
pub fn validate_input(input: &mut ConnectorInput) -> ConnectorResult<()> {
    if input.label.trim().is_empty()
        || input.label.len() > 120
        || input.label.chars().any(char::is_control)
        || input.scope.notion_page_ids.len() > 50
        || input.scope.azure_project_ids.len() > 50
    {
        return Err(ConnectorError::new(ConnectorErrorCode::InvalidSchema));
    }
    for id in input
        .scope
        .notion_page_ids
        .iter_mut()
        .chain(input.scope.azure_project_ids.iter_mut())
    {
        *id = canonical_id(id)?;
    }
    if let Some(id) = input.default_notion_page_id.as_mut() {
        *id = canonical_id(id)?;
        if !input.scope.notion_page_ids.contains(id) {
            return Err(ConnectorError::new(ConnectorErrorCode::PolicyDenied));
        }
    }
    match input.kind {
        ConnectorKind::Notion => {
            if input.organization.is_some()
                || input.tenant.is_some()
                || input.client_id.is_some()
                || !input.scope.azure_project_ids.is_empty()
            {
                return Err(ConnectorError::new(ConnectorErrorCode::InvalidSchema));
            }
        }
        ConnectorKind::AzureDevops => {
            segment(
                input
                    .organization
                    .as_deref()
                    .ok_or_else(|| ConnectorError::new(ConnectorErrorCode::InvalidSchema))?,
            )?;
            if let Some(tenant) = &input.tenant {
                if tenant != "organizations" && tenant != "common" {
                    canonical_id(tenant)?;
                }
            }
            if let Some(id) = input.client_id.as_mut() {
                *id = canonical_id(id)?;
            }
            if !input.scope.notion_page_ids.is_empty() || input.default_notion_page_id.is_some() {
                return Err(ConnectorError::new(ConnectorErrorCode::InvalidSchema));
            }
        }
    }
    Ok(())
}
pub fn require_project(config: &ConnectorConfig, project: &str) -> ConnectorResult<String> {
    let id = canonical_id(project)?;
    if config.kind != ConnectorKind::AzureDevops || !config.scope.azure_project_ids.contains(&id) {
        return Err(ConnectorError::new(ConnectorErrorCode::PolicyDenied));
    }
    Ok(id)
}
pub fn require_page(config: &ConnectorConfig, page: &str) -> ConnectorResult<String> {
    let id = canonical_id(page)?;
    if config.kind != ConnectorKind::Notion || !config.scope.notion_page_ids.contains(&id) {
        return Err(ConnectorError::new(ConnectorErrorCode::PolicyDenied));
    }
    Ok(id)
}
pub fn validate_defaults(
    defaults: &AzureDestinationDefaults,
    catalog: &AzureCatalog,
) -> ConnectorResult<()> {
    let has = |entries: &[CatalogEntry], id: &str| entries.iter().any(|e| e.id == id);
    let invalid = !has(&catalog.teams, &defaults.team_id)
        || defaults
            .backlog_id
            .as_deref()
            .is_some_and(|id| !has(&catalog.backlogs, id))
        || defaults.area_path.as_deref().is_some_and(|path| {
            !catalog
                .areas
                .iter()
                .any(|e| e.path.as_deref() == Some(path))
        })
        || defaults
            .work_item_type
            .as_deref()
            .is_some_and(|name| !catalog.work_item_types.iter().any(|e| e.name == name));
    if invalid {
        return Err(ConnectorError::new(ConnectorErrorCode::InvalidSchema));
    }
    match defaults.sprint_policy {
        SprintPolicy::Fixed => {
            if !catalog.iterations.iter().any(|e| {
                Some(&e.id) == defaults.iteration_id.as_ref()
                    && e.path.as_ref() == defaults.iteration_path.as_ref()
            }) {
                return Err(ConnectorError::new(ConnectorErrorCode::InvalidSchema));
            }
        }
        _ if defaults.iteration_id.is_some() || defaults.iteration_path.is_some() => {
            return Err(ConnectorError::new(ConnectorErrorCode::InvalidSchema))
        }
        _ => {}
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_path_and_url_in_org() {
        for value in ["../evil", "https://evil", "foo/bar", "foo?x", "", ".."] {
            assert!(segment(value).is_err());
        }
        assert!(segment("my-org").is_ok());
    }
    #[test]
    fn canonicalizes_hyphenless_ids() {
        assert_eq!(
            canonical_id("12345678123412341234123456789abc").unwrap(),
            "12345678-1234-1234-1234-123456789abc"
        );
    }
    #[test]
    fn fixed_sprint_must_be_real() {
        let d = AzureDestinationDefaults {
            project_id: "p".into(),
            team_id: "t".into(),
            backlog_id: None,
            area_path: None,
            work_item_type: None,
            sprint_policy: SprintPolicy::Fixed,
            iteration_id: Some("i".into()),
            iteration_path: Some("P\\I".into()),
        };
        assert!(validate_defaults(&d, &AzureCatalog::default()).is_err());
    }
}
