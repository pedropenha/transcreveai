//! Tauri commands for provider API keys (FR-011-01..05).
//!
//! These are deliberately write-only: no command returns a secret's value.
//! `secret_hint` exposes at most the last four characters.

use crate::commands::{CommandError, CommandErrorCode, CommandResult};
use crate::secrets;
use crate::settings;
use tauri::AppHandle;

fn known_provider(
    app: &AppHandle,
    provider_id: &str,
) -> CommandResult<settings::PostProcessProvider> {
    settings::get_settings(app)
        .post_process_providers
        .iter()
        .find(|p| p.id == provider_id)
        .cloned()
        .ok_or_else(|| {
            CommandError::new(
                CommandErrorCode::NotFound,
                format!("Provider '{provider_id}' not found"),
            )
        })
}

/// Store a provider API key in the OS credential vault. An empty value means
/// "remove the key" (the same thing clearing the field did when keys lived in
/// `settings.json`). Returns an optional non-blocking format warning
/// (FR-011-05); the key is saved regardless.
#[tauri::command]
#[specta::specta]
pub fn secret_set(
    app: AppHandle,
    provider_id: String,
    secret: String,
) -> CommandResult<Option<String>> {
    let provider = known_provider(&app, &provider_id)?;
    let secret = secret.trim().to_string();
    if secret.is_empty() {
        secrets::clear_provider_secret(&app, &provider_id).map_err(|e| {
            CommandError::logged(
                CommandErrorCode::Keyring,
                "Could not remove the key from the system credential vault",
                e,
            )
        })?;
        return Ok(None);
    }
    secrets::secret_store()
        .set(&provider_id, &secret)
        .map_err(|e| {
            CommandError::logged(
                CommandErrorCode::Keyring,
                "Could not save the key in the system credential vault",
                e,
            )
        })?;
    Ok(secrets::format_warning(
        &provider.id,
        &provider.label,
        &secret,
    ))
}

/// Delete a provider's key — vault entry plus any plaintext leftover still
/// pending migration (FR-011-04).
#[tauri::command]
#[specta::specta]
pub fn secret_clear(app: AppHandle, provider_id: String) -> CommandResult<()> {
    known_provider(&app, &provider_id)?;
    secrets::clear_provider_secret(&app, &provider_id).map_err(|e| {
        CommandError::logged(
            CommandErrorCode::Keyring,
            "Could not remove the key from the system credential vault",
            e,
        )
    })
}

/// Masked presence hint (`••••` + last four chars) — never the key itself
/// (FR-011-02).
#[tauri::command]
#[specta::specta]
pub fn secret_hint(app: AppHandle, provider_id: String) -> CommandResult<Option<String>> {
    known_provider(&app, &provider_id)?;
    let key = secrets::provider_api_key(&app, &provider_id);
    Ok(secrets::hint_for(key.as_deref()))
}
