//! `meeting_app_rules` CRUD commands (FR-008-15 "Lista de regras por app",
//! T-061). The rules-settings screen lists, adds, edits the `action` of, and
//! removes rules; builtin rows (Zoom/Teams/Meet/Webex seeds) accept
//! `set_action` but refuse `delete` — the "never ask" path is `ignore`.
//!
//! The detector thread re-reads the table every tick
//! (`meeting::run`'s `load_rules`), so writes here take effect without a
//! restart.

use super::{CommandError, CommandErrorCode, CommandResult};
use crate::db::meetings::{
    MeetingAppRule, MeetingAppRuleRepository, SqliteMeetingAppRuleRepository,
};
use rusqlite::Connection;
use tauri::AppHandle;

/// Open the app database for `meeting_app_rules` work. Shared by this module
/// and `commands::detector` (which flips rule actions from the toast).
pub(crate) fn open_rules_db(app: &AppHandle) -> CommandResult<Connection> {
    let dir = crate::portable::app_data_dir(app).map_err(|e| {
        CommandError::logged(
            CommandErrorCode::Internal,
            "Failed to resolve the app data directory",
            e,
        )
    })?;
    let path = crate::db::database_path(&dir).map_err(|e| {
        CommandError::logged(
            CommandErrorCode::Internal,
            "Failed to resolve the database path",
            e,
        )
    })?;
    crate::db::open_connection(&path).map_err(|e| {
        CommandError::logged(
            CommandErrorCode::Internal,
            "Failed to open the app database",
            e,
        )
    })
}

/// The `action` column accepts exactly `ask` | `auto_start` | `ignore`
/// (`RuleAction::parse`/`as_str` in `meeting::classifier` are the canonical
/// spellings).
fn validate_action(action: &str) -> CommandResult<()> {
    match action {
        "ask" | "auto_start" | "ignore" => Ok(()),
        _ => Err(CommandError::new(
            CommandErrorCode::InvalidInput,
            "Rule action must be 'ask', 'auto_start' or 'ignore'",
        )),
    }
}

fn find_rule(repo: &SqliteMeetingAppRuleRepository<'_>, id: &str) -> CommandResult<MeetingAppRule> {
    repo.list()
        .map_err(|e| {
            CommandError::logged(
                CommandErrorCode::Internal,
                "Failed to read the meeting app rules",
                e,
            )
        })?
        .into_iter()
        .find(|rule| rule.id == id)
        .ok_or_else(|| CommandError::new(CommandErrorCode::NotFound, "Meeting app rule not found"))
}

/// List every rule (builtin seeds first included) for the settings screen.
#[tauri::command]
#[specta::specta]
pub fn meeting_rules_list(app: AppHandle) -> CommandResult<Vec<MeetingAppRule>> {
    let conn = open_rules_db(&app)?;
    SqliteMeetingAppRuleRepository::new(&conn)
        .list()
        .map_err(|e| {
            CommandError::logged(
                CommandErrorCode::Internal,
                "Failed to read the meeting app rules",
                e,
            )
        })
}

/// Add a user rule. Browser exes (`chrome.exe`, `msedge.exe`, …) must carry a
/// `title_pattern` — the classifier can never fire them on the exe alone
/// (FR-008-02), so a pattern-less browser rule would be silently dead.
#[tauri::command]
#[specta::specta]
pub fn meeting_rule_add(
    app: AppHandle,
    exe: String,
    label: String,
    title_pattern: Option<String>,
    action: String,
) -> CommandResult<MeetingAppRule> {
    let exe = exe.trim().to_string();
    let label = label.trim().to_string();
    if exe.is_empty() || label.is_empty() {
        return Err(CommandError::new(
            CommandErrorCode::InvalidInput,
            "Rule exe and label must not be empty",
        ));
    }
    validate_action(&action)?;
    if let Some(pattern) = &title_pattern {
        regex::Regex::new(pattern).map_err(|e| {
            CommandError::logged(
                CommandErrorCode::InvalidInput,
                "The window title pattern is not a valid regex",
                e,
            )
        })?;
    }
    if title_pattern.is_none()
        && crate::meeting::BROWSER_EXES
            .iter()
            .any(|b| exe.eq_ignore_ascii_case(b))
    {
        return Err(CommandError::new(
            CommandErrorCode::InvalidInput,
            "Browser rules need a window title pattern to match a meeting",
        ));
    }

    let mut rule = MeetingAppRule::new(&exe, &label, &action);
    rule.title_pattern = title_pattern;

    let conn = open_rules_db(&app)?;
    SqliteMeetingAppRuleRepository::new(&conn)
        .create(&rule)
        .map_err(|e| {
            CommandError::logged(
                CommandErrorCode::Internal,
                "Failed to save the meeting app rule",
                e,
            )
        })?;
    Ok(rule)
}

/// Change a rule's action (`ask` | `auto_start` | `ignore`) — allowed on
/// builtin rows too ("always"/"never" from the toast land here via
/// `detector_respond`, the settings screen uses this command).
#[tauri::command]
#[specta::specta]
pub fn meeting_rule_set_action(app: AppHandle, id: String, action: String) -> CommandResult<()> {
    validate_action(&action)?;
    let conn = open_rules_db(&app)?;
    let repo = SqliteMeetingAppRuleRepository::new(&conn);
    find_rule(&repo, &id)?;
    repo.set_action(&id, &action).map_err(|e| {
        CommandError::logged(
            CommandErrorCode::Internal,
            "Failed to update the meeting app rule",
            e,
        )
    })
}

/// Remove a user rule. Builtin rows refuse deletion (`InvalidInput`) — the
/// supported "never ask again" path for them is `set_action(id, "ignore")`.
#[tauri::command]
#[specta::specta]
pub fn meeting_rule_delete(app: AppHandle, id: String) -> CommandResult<()> {
    let conn = open_rules_db(&app)?;
    let repo = SqliteMeetingAppRuleRepository::new(&conn);
    let rule = find_rule(&repo, &id)?;
    if rule.builtin {
        return Err(CommandError::new(
            CommandErrorCode::InvalidInput,
            "Built-in rules can't be removed; set them to 'ignore' instead",
        ));
    }
    repo.delete(&id).map_err(|e| {
        CommandError::logged(
            CommandErrorCode::Internal,
            "Failed to delete the meeting app rule",
            e,
        )
    })
}
