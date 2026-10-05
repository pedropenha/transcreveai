//! Operation journal (FR-013-12, NFR-013-05): lifecycle + durable
//! meeting↔remote links. Stores the reviewed payload for reconciliation;
//! never logs it and never stores secrets.
use super::types::*;
use rusqlite::{params, Connection, OptionalExtension};

fn db_error(error: rusqlite::Error) -> ConnectorError {
    ConnectorError {
        code: ConnectorErrorCode::TemporaryFailure,
        message: format!("connector.journal: {error}"),
    }
}

pub fn insert(
    conn: &Connection,
    record: &OperationRecord,
    draft: &AzureWorkItemDraft,
    fingerprint: &str,
) -> ConnectorResult<()> {
    let payload = serde_json::to_string(draft)
        .map_err(|_| ConnectorError::new(ConnectorErrorCode::InvalidSchema))?;
    conn.execute(
        "INSERT INTO connector_operations
            (id, connection_id, meeting_id, action, payload_json, fingerprint,
             status, remote_id, remote_url, error_code, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, NULL, NULL, ?8, ?8)",
        params![
            record.id,
            record.connection_id,
            record.meeting_id,
            record.action,
            payload,
            fingerprint,
            status_str(record.status),
            record.created_at,
        ],
    )
    .map_err(db_error)?;
    Ok(())
}

/// Full row: record + approved fingerprint + payload for execution.
pub fn get(
    conn: &Connection,
    id: &str,
) -> ConnectorResult<Option<(OperationRecord, String, AzureWorkItemDraft)>> {
    let row = conn
        .query_row(
            "SELECT id, connection_id, meeting_id, action, status, remote_id,
                    remote_url, error_code, created_at, updated_at,
                    fingerprint, payload_json
             FROM connector_operations WHERE id = ?1",
            params![id],
            |row| {
                Ok((
                    OperationRecord {
                        id: row.get(0)?,
                        connection_id: row.get(1)?,
                        meeting_id: row.get(2)?,
                        action: row.get(3)?,
                        status: parse_status(&row.get::<_, String>(4)?),
                        remote_id: row.get(5)?,
                        remote_url: row.get(6)?,
                        error_code: row.get(7)?,
                        created_at: row.get(8)?,
                        updated_at: row.get(9)?,
                    },
                    row.get::<_, String>(10)?,
                    row.get::<_, String>(11)?,
                ))
            },
        )
        .optional()
        .map_err(db_error)?;
    let Some((record, fingerprint, payload)) = row else {
        return Ok(None);
    };
    let draft: AzureWorkItemDraft = serde_json::from_str(&payload)
        .map_err(|_| ConnectorError::new(ConnectorErrorCode::InvalidSchema))?;
    Ok(Some((record, fingerprint, draft)))
}

/// Moves `awaiting_approval → executing`, once. `Ok(false)` means the row was
/// already claimed — callers must treat that as a concurrent execution and
/// reload instead of retrying the remote write (FR-013-12).
pub fn claim(conn: &Connection, id: &str, now: i64) -> ConnectorResult<bool> {
    let changed = conn
        .execute(
            "UPDATE connector_operations SET status = ?2, updated_at = ?3
             WHERE id = ?1 AND status = ?4",
            params![
                id,
                status_str(OperationStatus::Executing),
                now,
                status_str(OperationStatus::AwaitingApproval)
            ],
        )
        .map_err(db_error)?;
    Ok(changed == 1)
}

pub fn finish(
    conn: &Connection,
    id: &str,
    status: OperationStatus,
    remote_id: Option<&str>,
    remote_url: Option<&str>,
    error_code: Option<&str>,
    now: i64,
) -> ConnectorResult<()> {
    conn.execute(
        "UPDATE connector_operations
         SET status = ?2, remote_id = ?3, remote_url = ?4, error_code = ?5,
             updated_at = ?6
         WHERE id = ?1",
        params![
            id,
            status_str(status),
            remote_id,
            remote_url,
            error_code,
            now
        ],
    )
    .map_err(db_error)?;
    Ok(())
}

pub fn list_by_meeting(
    conn: &Connection,
    meeting_id: &str,
) -> ConnectorResult<Vec<OperationRecord>> {
    let mut stmt = conn
        .prepare(
            "SELECT id, connection_id, meeting_id, action, status, remote_id,
                    remote_url, error_code, created_at, updated_at
             FROM connector_operations
             WHERE meeting_id = ?1 ORDER BY created_at",
        )
        .map_err(db_error)?;
    let rows = stmt
        .query_map(params![meeting_id], |row| {
            Ok(OperationRecord {
                id: row.get(0)?,
                connection_id: row.get(1)?,
                meeting_id: row.get(2)?,
                action: row.get(3)?,
                status: parse_status(&row.get::<_, String>(4)?),
                remote_id: row.get(5)?,
                remote_url: row.get(6)?,
                error_code: row.get(7)?,
                created_at: row.get(8)?,
                updated_at: row.get(9)?,
            })
        })
        .map_err(db_error)?;
    rows.collect::<Result<_, _>>().map_err(db_error)
}

pub fn insert_link(
    conn: &Connection,
    meeting_id: &str,
    connection_id: &str,
    service: &str,
    remote_id: &str,
    remote_url: &str,
    operation_id: &str,
    now: i64,
) -> ConnectorResult<()> {
    conn.execute(
        "INSERT INTO meeting_remote_links
            (meeting_id, connection_id, service, remote_id, remote_url,
             operation_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            meeting_id,
            connection_id,
            service,
            remote_id,
            remote_url,
            operation_id,
            now
        ],
    )
    .map_err(db_error)?;
    Ok(())
}

fn status_str(status: OperationStatus) -> &'static str {
    match status {
        OperationStatus::AwaitingApproval => "awaiting_approval",
        OperationStatus::Executing => "executing",
        OperationStatus::Succeeded => "succeeded",
        OperationStatus::Failed => "failed",
        OperationStatus::OutcomeUnknown => "outcome_unknown",
        OperationStatus::Cancelled => "cancelled",
    }
}

fn parse_status(value: &str) -> OperationStatus {
    match value {
        "executing" => OperationStatus::Executing,
        "succeeded" => OperationStatus::Succeeded,
        "failed" => OperationStatus::Failed,
        "outcome_unknown" => OperationStatus::OutcomeUnknown,
        "cancelled" => OperationStatus::Cancelled,
        _ => OperationStatus::AwaitingApproval,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> Connection {
        let mut conn = Connection::open_in_memory().expect("open in-memory db");
        crate::db::configure_connection(&conn).expect("configure");
        crate::db::run_migrations(&mut conn).expect("run migrations");
        conn
    }

    fn draft() -> AzureWorkItemDraft {
        AzureWorkItemDraft {
            project_id: "proj-1".into(),
            work_item_type: "Task".into(),
            title: "Ship it".into(),
            description: "Details".into(),
            area_path: None,
            iteration_path: None,
            parent_id: Some(42),
        }
    }

    fn record(id: &str) -> OperationRecord {
        OperationRecord {
            id: id.into(),
            connection_id: "conn-1".into(),
            meeting_id: Some("mtg-1".into()),
            action: "azure_create_work_item".into(),
            status: OperationStatus::AwaitingApproval,
            remote_id: None,
            remote_url: None,
            error_code: None,
            created_at: 100,
            updated_at: 100,
        }
    }

    #[test]
    fn insert_get_roundtrip_preserves_draft_and_fingerprint() {
        let conn = setup();
        insert(&conn, &record("op-1"), &draft(), "fp-abc").unwrap();
        let (loaded, fp, loaded_draft) = get(&conn, "op-1").unwrap().unwrap();
        assert_eq!(fp, "fp-abc");
        assert_eq!(loaded.status, OperationStatus::AwaitingApproval);
        assert_eq!(loaded_draft.title, "Ship it");
        assert_eq!(loaded_draft.parent_id, Some(42));
        assert!(get(&conn, "missing").unwrap().is_none());
    }

    #[test]
    fn claim_is_single_use() {
        let conn = setup();
        insert(&conn, &record("op-2"), &draft(), "fp").unwrap();
        assert!(claim(&conn, "op-2", 200).unwrap());
        assert!(!claim(&conn, "op-2", 201).unwrap());
        assert_eq!(
            get(&conn, "op-2").unwrap().unwrap().0.status,
            OperationStatus::Executing
        );
    }

    #[test]
    fn finish_persists_outcome_and_links_meeting() {
        let conn = setup();
        insert(&conn, &record("op-3"), &draft(), "fp").unwrap();
        claim(&conn, "op-3", 200).unwrap();
        finish(
            &conn,
            "op-3",
            OperationStatus::Succeeded,
            Some("777"),
            Some("https://dev.azure.com/o/p/_workitems/edit/777"),
            None,
            300,
        )
        .unwrap();
        insert_link(
            &conn,
            "mtg-1",
            "conn-1",
            "azure_devops",
            "777",
            "https://dev.azure.com/o/p/_workitems/edit/777",
            "op-3",
            300,
        )
        .unwrap();
        let ops = list_by_meeting(&conn, "mtg-1").unwrap();
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].status, OperationStatus::Succeeded);
        assert_eq!(ops[0].remote_id.as_deref(), Some("777"));
        let links: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM meeting_remote_links WHERE meeting_id = 'mtg-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(links, 1);
    }

    #[test]
    fn finish_failure_records_error_code() {
        let conn = setup();
        insert(&conn, &record("op-4"), &draft(), "fp").unwrap();
        claim(&conn, "op-4", 1).unwrap();
        finish(
            &conn,
            "op-4",
            OperationStatus::Failed,
            None,
            None,
            Some("PermissionDenied"),
            2,
        )
        .unwrap();
        let ops = list_by_meeting(&conn, "mtg-1").unwrap();
        assert_eq!(ops[0].status, OperationStatus::Failed);
        assert_eq!(ops[0].error_code.as_deref(), Some("PermissionDenied"));
    }
}
