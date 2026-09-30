//! Provider API keys live in the operating system's credential vault —
//! Windows Credential Manager, macOS Keychain, freedesktop Secret Service on
//! Linux — and never in `settings.json` (FR-011-01).
//!
//! Deliberate asymmetry (FR-011-02): the frontend can only *write* keys
//! (`secret_set`), clear them (`secret_clear`) and read a masked hint
//! (`secret_hint`). There is no Tauri command that returns a key's value.
//!
//! Migration contract (T-016): plaintext `post_process_api_keys` entries from
//! the previous release move into the vault on settings load. A key is only
//! removed from the JSON after the vault confirms the write, so a temporarily
//! unavailable credential store can never strand a working key; the migration
//! simply retries on the next load (or on the next internal read).
//!
//! Additional guarantees: secrets must never reach logs or error messages
//! (FR-011-03) — [`redact_sensitive`] rewrites log output — and the only
//! non-vault backend is an opt-in in-memory store for tests/CI.

use log::{debug, warn};
use serde_json::Value;
use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, OnceLock};
use tauri::AppHandle;
use tauri_plugin_store::StoreExt;

/// Settings-store field that used to hold per-provider API keys in plaintext.
/// Reads of it are migration leftovers only — it is never written by the app.
pub(crate) const LEGACY_API_KEYS_FIELD: &str = "post_process_api_keys";

/// Vault entry namespace. The stored entry name for provider `<id>` is
/// `Transcreve.ai/provider/<id>` on every platform (FR-011-01).
pub const SECRET_SCOPE_PROVIDER: &str = "Transcreve.ai/provider";

/// Errors carry only operational context — a secret value is never embedded
/// in a message, matching FR-011-03.
#[derive(Debug)]
pub struct SecretError(String);

impl fmt::Display for SecretError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SecretError {}

/// Abstraction over the secret backend so tests and CI can swap in
/// [`MemorySecretStore`] (FR-011-01).
pub trait SecretStore: Send + Sync {
    fn get(&self, provider_id: &str) -> Result<Option<String>, SecretError>;
    fn set(&self, provider_id: &str, secret: &str) -> Result<(), SecretError>;
    fn delete(&self, provider_id: &str) -> Result<(), SecretError>;
}

/// The operating system's credential vault.
pub struct OsSecretStore;

/// Full vault entry name: `Transcreve.ai/provider/<provider_id>`.
pub fn vault_entry_name(provider_id: &str) -> String {
    format!("{SECRET_SCOPE_PROVIDER}/{provider_id}")
}

fn validate_provider_id(provider_id: &str) -> Result<(), SecretError> {
    let ok = !provider_id.trim().is_empty()
        && !provider_id.contains(['/', '\\'])
        && !provider_id.chars().any(char::is_control);
    if ok {
        Ok(())
    } else {
        Err(SecretError(
            "invalid provider id for a vault entry".to_string(),
        ))
    }
}

/// Map backend failures to fixed, secret-free descriptions. `BadEncoding`
/// specifically carries the raw stored bytes in its payload — it must never
/// be stringified into an error or log (FR-011-03).
fn entry_error(context: &str, error: keyring_core::Error) -> SecretError {
    let detail = match &error {
        keyring_core::Error::NoEntry => "entry not found".to_string(),
        keyring_core::Error::Ambiguous(_) => "multiple entries matched".to_string(),
        // These variants carry the raw stored bytes — never stringify them.
        keyring_core::Error::BadEncoding(_) => {
            "stored data is not a valid UTF-8 secret".to_string()
        }
        keyring_core::Error::BadDataFormat(..) => "stored data is malformed".to_string(),
        keyring_core::Error::BadStoreFormat(_) => "vault store is malformed".to_string(),
        keyring_core::Error::TooLong(..) => "secret exceeds store size limit".to_string(),
        keyring_core::Error::Invalid(..) => "invalid entry attributes".to_string(),
        keyring_core::Error::NoDefaultStore => "no credential store configured".to_string(),
        keyring_core::Error::NotSupportedByStore(_) => {
            "operation not supported by this store".to_string()
        }
        keyring_core::Error::PlatformFailure(inner) => {
            format!("credential store error: {inner}")
        }
        keyring_core::Error::NoStorageAccess(inner) => {
            format!("cannot access credential store: {inner}")
        }
        _ => "credential store error".to_string(),
    };
    SecretError(format!("{context}: {detail}"))
}

fn vault_entry(provider_id: &str) -> Result<keyring_core::Entry, SecretError> {
    validate_provider_id(provider_id)?;
    // `keyring::Entry::store_status` performs the one-time default-store
    // initialization (it also backs `keyring_core::Entry::new`).
    if let Err(e) = keyring::Entry::store_status() {
        return Err(SecretError(format!("OS credential store unavailable: {e}")));
    }
    let name = vault_entry_name(provider_id);
    let result = {
        #[cfg(target_os = "windows")]
        {
            // The Windows store otherwise composes the credential target name
            // as "<user>.<service>"; an explicit `target` makes it exactly
            // "Transcreve.ai/provider/<provider_id>" as required.
            let modifiers = HashMap::from([("target", name.as_str())]);
            keyring_core::Entry::new_with_modifiers(&name, provider_id, &modifiers)
        }
        #[cfg(not(target_os = "windows"))]
        {
            keyring_core::Entry::new(&name, provider_id)
        }
    };
    result.map_err(|e| entry_error("failed to open vault entry", e))
}

impl SecretStore for OsSecretStore {
    fn get(&self, provider_id: &str) -> Result<Option<String>, SecretError> {
        match vault_entry(provider_id)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(e) => Err(entry_error("failed to read provider key", e)),
        }
    }

    fn set(&self, provider_id: &str, secret: &str) -> Result<(), SecretError> {
        vault_entry(provider_id)?
            .set_password(secret)
            .map_err(|e| entry_error("failed to save provider key", e))
    }

    fn delete(&self, provider_id: &str) -> Result<(), SecretError> {
        match vault_entry(provider_id)?.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(e) => Err(entry_error("failed to delete provider key", e)),
        }
    }
}

/// In-memory store for unit tests and CI-only dev builds. State is process
/// local and never touches disk or the OS vault.
#[derive(Debug, Default)]
pub struct MemorySecretStore {
    inner: Mutex<HashMap<String, String>>,
}

impl SecretStore for MemorySecretStore {
    fn get(&self, provider_id: &str) -> Result<Option<String>, SecretError> {
        validate_provider_id(provider_id)?;
        Ok(self
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(provider_id)
            .cloned())
    }

    fn set(&self, provider_id: &str, secret: &str) -> Result<(), SecretError> {
        validate_provider_id(provider_id)?;
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(provider_id.to_string(), secret.to_string());
        Ok(())
    }

    fn delete(&self, provider_id: &str) -> Result<(), SecretError> {
        validate_provider_id(provider_id)?;
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(provider_id);
        Ok(())
    }
}

/// Store whose writes/delete always fail — exercises the "vault unavailable"
/// migration path.
#[cfg(test)]
#[derive(Debug)]
pub struct FailingSecretStore;

#[cfg(test)]
impl SecretStore for FailingSecretStore {
    fn get(&self, _provider_id: &str) -> Result<Option<String>, SecretError> {
        Err(SecretError("store unavailable".to_string()))
    }
    fn set(&self, _provider_id: &str, _secret: &str) -> Result<(), SecretError> {
        Err(SecretError("store unavailable".to_string()))
    }
    fn delete(&self, _provider_id: &str) -> Result<(), SecretError> {
        Err(SecretError("store unavailable".to_string()))
    }
}

/// The store implementation in use. In dev/test builds only,
/// `TRANSCREVE_SECRETS_MEMORY_STORE` swaps in the process-local store so
/// tests and CI can run without an OS credential service (the only
/// environment-variable backend, per FR-011-01).
pub fn secret_store() -> Arc<dyn SecretStore> {
    #[cfg(debug_assertions)]
    {
        if crate::utils::env_flag_enabled("TRANSCREVE_SECRETS_MEMORY_STORE") {
            static MEM: OnceLock<Arc<MemorySecretStore>> = OnceLock::new();
            return MEM
                .get_or_init(|| Arc::new(MemorySecretStore::default()))
                .clone();
        }
    }
    static OS: OnceLock<Arc<OsSecretStore>> = OnceLock::new();
    OS.get_or_init(|| Arc::new(OsSecretStore)).clone()
}

/// Masked display string: `••••` plus at most the last four characters. Keys
/// of four characters or fewer reveal no characters at all, so the hint can
/// never carry enough information to reconstruct a secret (FR-011-02).
pub fn hint_for(secret: Option<&str>) -> Option<String> {
    let secret = secret.filter(|s| !s.is_empty())?;
    let len = secret.chars().count();
    if len > 4 {
        let suffix: String = secret.chars().skip(len - 4).collect();
        Some(format!("••••{suffix}"))
    } else {
        Some("••••".to_string())
    }
}

/// Expected key prefixes for providers with a stable public format. Unknown
/// or self-hosted providers return `None` (nothing to check against).
fn expected_key_prefix(provider_id: &str) -> Option<&'static str> {
    match provider_id {
        "openai" => Some("sk-"),
        "anthropic" => Some("sk-ant-"),
        "openrouter" => Some("sk-or-"),
        "groq" => Some("gsk_"),
        "cerebras" => Some("csk-"),
        _ => None,
    }
}

/// Non-blocking format check (FR-011-05): a warning when the key doesn't
/// match the provider's known prefix, `None` when it does or there is no
/// known format.
pub fn format_warning(provider_id: &str, provider_label: &str, key: &str) -> Option<String> {
    let prefix = expected_key_prefix(provider_id)?;
    if key.starts_with(prefix) {
        None
    } else {
        Some(format!(
            "Saved anyway, but {provider_label} keys usually start with \"{prefix}\" — double-check the key."
        ))
    }
}

/// Move plaintext `post_process_api_keys` entries into the vault and strip
/// them from `settings_json`. A key is removed only after the vault accepts
/// it — anything the store rejects stays in place so it is never lost, and
/// the migration retries on the next load. Returns true when the JSON
/// changed.
pub(crate) fn migrate_plaintext_api_keys(
    settings_json: &mut Value,
    store: &dyn SecretStore,
) -> bool {
    let mut changed = false;
    let Some(map) = settings_json
        .get_mut(LEGACY_API_KEYS_FIELD)
        .and_then(Value::as_object_mut)
    else {
        return false;
    };

    let entries: Vec<(String, String)> = map
        .iter()
        .filter_map(|(id, v)| v.as_str().map(|s| (id.clone(), s.to_string())))
        .collect();

    for (provider_id, key) in entries {
        if key.is_empty() {
            map.remove(&provider_id);
            changed = true;
            continue;
        }
        match store.set(&provider_id, &key) {
            Ok(()) => {
                map.remove(&provider_id);
                changed = true;
                debug!("Migrated '{provider_id}' API key into the OS credential vault");
            }
            Err(e) => {
                warn!(
                    "Could not move '{provider_id}' API key into the OS vault ({e}); \
                     keeping it in settings until the vault accepts it"
                );
            }
        }
    }

    if map.is_empty() {
        if let Some(obj) = settings_json.as_object_mut() {
            obj.remove(LEGACY_API_KEYS_FIELD);
        }
        changed = true;
    }
    changed
}

/// Re-attach pending `post_process_api_keys` entries to an outgoing settings
/// JSON so routine settings writes never drop keys whose vault write has not
/// succeeded yet.
pub(crate) fn reattach_pending_api_keys(out: &mut Value, pending: Option<&Value>) {
    let pending = pending.and_then(Value::as_object).filter(|m| !m.is_empty());
    if let (Some(pending), Some(obj)) = (pending, out.as_object_mut()) {
        obj.insert(
            LEGACY_API_KEYS_FIELD.to_string(),
            Value::Object(pending.clone()),
        );
    }
}

/// Read a still-unmigrated plaintext entry straight from the store JSON. Only
/// used while a vault write has not succeeded, so a temporarily unavailable
/// credential store never strands a working key.
fn legacy_pending_api_key(app: &AppHandle, provider_id: &str) -> Option<String> {
    let store = app
        .store(crate::portable::store_path(
            crate::settings::SETTINGS_STORE_PATH,
        ))
        .ok()?;
    let key = store
        .get("settings")?
        .get(LEGACY_API_KEYS_FIELD)?
        .get(provider_id)?
        .as_str()?
        .to_string();
    if key.is_empty() {
        None
    } else {
        Some(key)
    }
}

fn remove_pending_api_key(app: &AppHandle, provider_id: &str) -> Result<(), SecretError> {
    let store = app
        .store(crate::portable::store_path(
            crate::settings::SETTINGS_STORE_PATH,
        ))
        .map_err(|e| SecretError(format!("settings store unavailable: {e}")))?;
    let Some(mut settings) = store.get("settings") else {
        return Ok(());
    };
    let Some(map) = settings
        .get_mut(LEGACY_API_KEYS_FIELD)
        .and_then(Value::as_object_mut)
    else {
        return Ok(());
    };
    map.remove(provider_id);
    if map.is_empty() {
        if let Some(obj) = settings.as_object_mut() {
            obj.remove(LEGACY_API_KEYS_FIELD);
        }
    }
    store.set("settings", settings);
    Ok(())
}

/// Internal read path for provider keys — there is intentionally no Tauri
/// command exposing it. Prefers the OS vault; falls back to a not-yet
/// migrated plaintext copy so a temporarily unavailable vault cannot strand
/// a working key, and re-attempts the vault write opportunistically.
pub fn provider_api_key(app: &AppHandle, provider_id: &str) -> Option<String> {
    match secret_store().get(provider_id) {
        Ok(Some(key)) if !key.is_empty() => return Some(key),
        Ok(_) => {}
        Err(e) => warn!("Could not read '{provider_id}' key from the OS vault: {e}"),
    }

    let pending = legacy_pending_api_key(app, provider_id)?;
    // Opportunistic heal: retry the vault write and drop the plaintext copy as
    // soon as it succeeds instead of waiting for the next settings load.
    if secret_store().set(provider_id, &pending).is_ok() {
        if let Err(e) = remove_pending_api_key(app, provider_id) {
            warn!("'{provider_id}' is now vaulted but removing its plaintext copy failed: {e}");
        }
        debug!("Moved '{provider_id}' API key into the OS vault on read");
    }
    Some(pending)
}

/// Delete a provider's secret — the vault entry plus any plaintext leftover
/// still pending migration (FR-011-04).
pub fn clear_provider_secret(app: &AppHandle, provider_id: &str) -> Result<(), SecretError> {
    secret_store().delete(provider_id)?;
    remove_pending_api_key(app, provider_id)
}

const REDACTION_PATTERN_COUNT: usize = 6;

fn redaction_regexes() -> &'static [regex::Regex] {
    const PATTERNS: &[&str] = &[
        // OpenAI-family keys — sk-, sk-proj-, sk-ant-, sk-or-, sk-svcacct-, csk-
        r#"[A-Za-z0-9_-]*sk-[A-Za-z0-9_-]{8,}"#,
        r#"gsk_[A-Za-z0-9_-]{8,}"#,
        r#"csk-[A-Za-z0-9_-]{8,}"#,
        // Authorization headers
        r#"(?i)bearer\s+[A-Za-z0-9._~+/=-]{8,}"#,
        r#"(?i)x-api-key["'\s]*[:=]\s*["']?[A-Za-z0-9._~+/=-]{8,}"#,
        // generic `api_key=` / `apikey: ` style parameters
        r#"(?i)api[-_]?key["'\s]*[:=]\s*["']?[A-Za-z0-9._~+/=-]{8,}"#,
    ];
    static RES: OnceLock<Vec<regex::Regex>> = OnceLock::new();
    RES.get_or_init(|| {
        PATTERNS
            .iter()
            .filter_map(|p| regex::Regex::new(p).ok())
            .collect()
    })
}

/// Rewrite secret-looking material in a log line before it can reach any
/// output — files, stdout, or the webview log event (FR-011-03). Patterns are
/// tested by unit tests; a malformed pattern is skipped rather than panicking
/// inside the logging path.
pub fn redact_sensitive(message: &str) -> String {
    let mut out: Cow<'_, str> = Cow::Borrowed(message);
    for re in redaction_regexes() {
        out = Cow::Owned(re.replace_all(&out, "<redacted>").into_owned());
    }
    debug_assert_eq!(
        redaction_regexes().len(),
        REDACTION_PATTERN_COUNT,
        "a redaction pattern failed to compile — secrets could leak to logs"
    );
    out.into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn memory_store_set_get_delete_roundtrip() {
        let store = MemorySecretStore::default();
        assert_eq!(store.get("openai").unwrap(), None);
        store.set("openai", "sk-test-value").unwrap();
        assert_eq!(
            store.get("openai").unwrap().as_deref(),
            Some("sk-test-value")
        );
        store.set("openai", "sk-other").unwrap();
        assert_eq!(store.get("openai").unwrap().as_deref(), Some("sk-other"));
        store.delete("openai").unwrap();
        assert_eq!(store.get("openai").unwrap(), None);
        store.delete("openai").unwrap(); // idempotent
    }

    #[test]
    fn vault_entry_names_match_the_spec_shape() {
        assert_eq!(vault_entry_name("openai"), "Transcreve.ai/provider/openai");
        assert!(validate_provider_id("evil/../name").is_err());
        assert!(validate_provider_id("").is_err());
        assert!(validate_provider_id("with\nnewline").is_err());
        assert!(validate_provider_id("custom").is_ok());
    }

    #[test]
    fn hint_never_reveals_more_than_four_characters() {
        assert_eq!(hint_for(None), None);
        assert_eq!(hint_for(Some("")), None);
        assert_eq!(hint_for(Some("abcd")), Some("••••".to_string()));
        assert_eq!(hint_for(Some("abcde")), Some("••••bcde".to_string()));
        assert_eq!(hint_for(Some("sk-proj-xyzw")), Some("••••xyzw".to_string()));
        let hint = hint_for(Some("sk-proj-123456789")).unwrap();
        assert_eq!(hint, "••••6789");
        assert!(!hint.contains("sk-"), "hint must not leak key material");
    }

    #[test]
    fn migration_moves_keys_into_the_store_and_strips_json() {
        let store = MemorySecretStore::default();
        let mut json = json!({
            "post_process_provider_id": "openai",
            "post_process_api_keys": {"openai": "sk-live-1", "groq": "gsk_live-2", "empty": ""},
        });
        assert!(migrate_plaintext_api_keys(&mut json, &store));
        assert!(json.get("post_process_api_keys").is_none());
        assert_eq!(
            store.get("openai").unwrap().as_deref(),
            Some("sk-live-1"),
            "existing keys must be preserved without loss"
        );
        assert_eq!(store.get("groq").unwrap().as_deref(), Some("gsk_live-2"));
        // second run is a no-op
        assert!(!migrate_plaintext_api_keys(&mut json, &store));
    }

    #[test]
    fn migration_keeps_pending_keys_when_the_vault_fails() {
        let store = FailingSecretStore;
        let mut json = json!({
            "post_process_api_keys": {"openai": "sk-keep-me"},
        });
        assert!(!migrate_plaintext_api_keys(&mut json, &store));
        assert_eq!(
            json["post_process_api_keys"]["openai"].as_str(),
            Some("sk-keep-me"),
            "a failed vault write must never drop the key"
        );
    }

    #[test]
    fn migration_preserves_non_string_leftovers() {
        let store = MemorySecretStore::default();
        let mut json = json!({
            "post_process_api_keys": {"weird": 42, "openai": "sk-x1"},
        });
        assert!(migrate_plaintext_api_keys(&mut json, &store));
        let left = json["post_process_api_keys"].as_object().unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left["weird"].as_i64(), Some(42));
    }

    #[test]
    fn serialized_settings_carry_no_secret_field() {
        let settings = crate::settings::AppSettings::default();
        let json = serde_json::to_value(&settings).unwrap();
        assert!(json.get("post_process_api_keys").is_none());
    }

    #[test]
    fn reattach_preserves_only_pending_entries() {
        let mut out = json!({"post_process_provider_id": "groq"});
        reattach_pending_api_keys(&mut out, Some(&json!({"openai": "sk-pending"})));
        assert_eq!(out["post_process_api_keys"]["openai"], "sk-pending");

        let mut out2 = json!({});
        reattach_pending_api_keys(&mut out2, Some(&json!({})));
        reattach_pending_api_keys(&mut out2, None);
        assert!(out2.get("post_process_api_keys").is_none());
    }

    #[test]
    fn format_warning_flags_unusual_but_saves_anyway() {
        assert!(format_warning("openai", "OpenAI", "sk-abc").is_none());
        let w = format_warning("openai", "OpenAI", "abc-not-a-key").unwrap();
        assert!(w.contains("sk-"));
        assert!(w.contains("Saved anyway"));
        assert!(format_warning("custom", "Custom", "anything").is_none());
    }

    #[test]
    fn redaction_covers_required_patterns() {
        let cases = [
            "Authorization: Bearer abcdefgh12345",
            "x-api-key: abcdef1234567890",
            "x-api-key=\"ABCDEFGH12345\"",
            "key = sk-proj-AbCdEf123456",
            "gsk_AbCdEfGhIjKlMnOpQrSt",
            "api_key=sk-svcacct-abcdefghi",
        ];
        for case in cases {
            let redacted = redact_sensitive(case);
            assert!(redacted.contains("<redacted>"), "{case}");
            assert!(
                !redacted.contains("abc"),
                "secret material must not survive: {redacted}"
            );
        }
    }

    #[test]
    fn redaction_leaves_normal_text_alone() {
        let msg = "Loading model whisper-base for provider openai";
        assert_eq!(redact_sensitive(msg), msg);
    }
}
