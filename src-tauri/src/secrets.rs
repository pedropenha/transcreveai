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
//! removed from the JSON after the vault confirms the write — and a vault
//! entry that already holds a key always wins over the plaintext copy (the
//! migration discards the leftover rather than overwriting a newer key).
//! Failed writes keep the plaintext pending so a temporarily unavailable
//! credential store can never strand a working key; retries are throttled by
//! an exponential backoff so a down vault doesn't stall every settings read.
//!
//! Additional guarantees: secrets must never reach logs or error messages
//! (FR-011-03) — [`redact_sensitive`] rewrites log output — and the only
//! non-vault backend is an opt-in in-memory store for tests/CI. Because that
//! store is process-local, migration and the read-path heal are skipped while
//! `TRANSCREVE_SECRETS_MEMORY_STORE` is active: "moving" a key into a volatile
//! store and deleting the plaintext would silently lose it on restart.

use log::{debug, warn};
use serde_json::Value;
use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
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
/// local and never touches disk or the OS vault. Only reachable under
/// `debug_assertions` (see `secret_store`), so it is compiled out of release
/// app builds. Unit tests can instantiate it in either build profile.
#[cfg(any(debug_assertions, test))]
#[derive(Default)]
pub struct MemorySecretStore {
    inner: Mutex<HashMap<String, String>>,
}

// Manual Debug so a log/inspect slip can never dump stored key material.
#[cfg(any(debug_assertions, test))]
impl fmt::Debug for MemorySecretStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemorySecretStore").finish_non_exhaustive()
    }
}

#[cfg(any(debug_assertions, test))]
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

/// Whether the process-local volatile store is the active backend. Only
/// possible in dev/test builds (`TRANSCREVE_SECRETS_MEMORY_STORE`): anything
/// "migrated" into it would vanish on restart, so while it is active the
/// plaintext-to-vault migration and the read-path heal are skipped entirely —
/// pending plaintext stays in settings.json until a real vault is in use.
#[cfg(debug_assertions)]
fn memory_store_active() -> bool {
    crate::utils::env_flag_enabled("TRANSCREVE_SECRETS_MEMORY_STORE")
}

#[cfg(not(debug_assertions))]
fn memory_store_active() -> bool {
    false
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

/// Masked display string: `••••` plus a trailing suffix of at most a quarter
/// of the key's length, capped at four characters. A short key therefore
/// reveals proportionally less (a 5–7 char key reveals 1 char, never the old
/// fixed 4), so the hint can never carry enough information to reconstruct a
/// secret (FR-011-02).
pub fn hint_for(secret: Option<&str>) -> Option<String> {
    let secret = secret.filter(|s| !s.is_empty())?;
    let len = secret.chars().count();
    let reveal = (len / 4).min(4);
    if reveal == 0 {
        return Some("••••".to_string());
    }
    let suffix: String = secret.chars().skip(len - reveal).collect();
    Some(format!("••••{suffix}"))
}

/// Hard input ceiling for [`commands::secret_set`]: Windows Credential
/// Manager blobs cap at 2560 bytes, and control characters (a pasted
/// newline, NUL, …) are never legitimate key material.
pub const MAX_SECRET_BYTES: usize = 2560;

/// Reject input the OS vault cannot store or that can never be a real key.
pub fn validate_secret(secret: &str) -> Result<(), SecretError> {
    if secret.len() > MAX_SECRET_BYTES {
        return Err(SecretError(format!(
            "key is too long for the OS credential vault (max {MAX_SECRET_BYTES} bytes)"
        )));
    }
    if secret.chars().any(char::is_control) {
        return Err(SecretError(
            "key contains control characters (e.g. a pasted newline)".to_string(),
        ));
    }
    Ok(())
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
/// the migration retries on the next load. A vault entry that already holds
/// a non-empty key always wins: the plaintext leftover is then dropped
/// without touching the vault, so a stale pending key can never overwrite a
/// newer one saved since. Non-string leftovers are dropped outright — they
/// were never usable keys. Returns true when the JSON changed.
pub(crate) fn migrate_plaintext_api_keys(
    settings_json: &mut Value,
    store: &dyn SecretStore,
) -> bool {
    if memory_store_active() {
        // The in-memory backend is process-local (CI/dev only): "migrating"
        // into it and deleting the plaintext would lose the key on restart.
        static WARNED: OnceLock<()> = OnceLock::new();
        WARNED.get_or_init(|| {
            warn!("TRANSCREVE_SECRETS_MEMORY_STORE is active — skipping plaintext API key migration (the in-memory store is volatile)");
        });
        return false;
    }

    let mut changed = false;
    let Some(map) = settings_json
        .get_mut(LEGACY_API_KEYS_FIELD)
        .and_then(Value::as_object_mut)
    else {
        // A non-object leftover can never be migrated — drop it outright
        // rather than letting it ride along on settings writes forever.
        if settings_json.get(LEGACY_API_KEYS_FIELD).is_some() {
            if let Some(obj) = settings_json.as_object_mut() {
                obj.remove(LEGACY_API_KEYS_FIELD);
            }
            return true;
        }
        return false;
    };

    let entries: Vec<(String, Option<String>)> = map
        .iter()
        .map(|(id, v)| (id.clone(), v.as_str().map(|s| s.trim().to_string())))
        .collect();

    for (provider_id, key) in entries {
        let Some(key) = key.filter(|k| !k.is_empty()) else {
            // Empty strings and non-string leftovers are dropped outright.
            map.remove(&provider_id);
            changed = true;
            continue;
        };
        match store.get(&provider_id) {
            // The vault already holds a key — it is newer than this pending
            // plaintext copy (a `secret_set` landed after the vault write
            // that stranded it failed). The vault wins; discard the stale
            // plaintext without writing.
            Ok(Some(existing)) if !existing.is_empty() => {
                map.remove(&provider_id);
                changed = true;
                debug!(
                    "Dropped pending '{provider_id}' plaintext key: the OS vault already holds one"
                );
            }
            // Vault state unknown — a blind `set` could overwrite a key we
            // could not see. Keep the plaintext pending and retry later.
            Err(e) => {
                warn!(
                    "Could not read the OS vault before migrating '{provider_id}' ({e}); \
                     keeping the plaintext key until the vault accepts it"
                );
            }
            Ok(_) => match store.set(&provider_id, &key) {
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
            },
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

/// Drop a still-unmigrated plaintext copy for `provider_id` from the settings
/// blob. Called after a successful vault write (migration, `secret_set`, the
/// read-path heal) and by `secret_clear`. Best-effort: callers warn-log a
/// failure — the vaulted key is already durable and a leftover is discarded
/// by the next migration pass anyway (the vault wins).
///
/// The settings-blob lock serializes this read-modify-write with
/// `get_settings`/`write_settings` so a concurrent settings write can never
/// resurrect the plaintext or lose the removal.
pub(crate) fn remove_pending_api_key(
    app: &AppHandle,
    provider_id: &str,
) -> Result<(), SecretError> {
    let _blob_guard = crate::settings::lock_settings_blob();
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

/// Vault-write retry throttle for the best-effort paths (settings-load
/// migration and the read-path heal). A down OS credential store is usually
/// durable for a while; without throttling, every `get_settings` call would
/// synchronously hit it and log the same warning again.
static VAULT_WRITE_RETRY: Mutex<Option<VaultWriteRetry>> = Mutex::new(None);

struct VaultWriteRetry {
    failures: u32,
    next_attempt: Instant,
}

const VAULT_RETRY_BASE: Duration = Duration::from_secs(1);
const VAULT_RETRY_MAX: Duration = Duration::from_secs(5 * 60);

/// Whether another best-effort vault write may be attempted now. The first
/// call is always allowed; each recorded failure pushes the next attempt back
/// exponentially (1s → 2s → … → 5min cap) until a success resets the schedule.
pub(crate) fn vault_write_retry_allowed() -> bool {
    let guard = VAULT_WRITE_RETRY.lock().unwrap_or_else(|e| e.into_inner());
    match &*guard {
        None => true,
        Some(retry) => Instant::now() >= retry.next_attempt,
    }
}

/// Record the outcome of a best-effort vault write. `succeeded` means either
/// the write landed or there was nothing pending — anything else backs off.
pub(crate) fn vault_write_retry_record(succeeded: bool) {
    let mut guard = VAULT_WRITE_RETRY.lock().unwrap_or_else(|e| e.into_inner());
    if succeeded {
        *guard = None;
        return;
    }
    let failures = guard.as_ref().map_or(0, |r| r.failures).saturating_add(1);
    let delay = VAULT_RETRY_BASE
        .saturating_mul(1u32 << (failures.saturating_sub(1)).min(8))
        .min(VAULT_RETRY_MAX);
    *guard = Some(VaultWriteRetry {
        failures,
        next_attempt: Instant::now() + delay,
    });
}

/// Internal read path for provider keys — there is intentionally no Tauri
/// command exposing it. Prefers the OS vault; falls back to a not-yet
/// migrated plaintext copy so a temporarily unavailable vault cannot strand
/// a working key, and re-attempts the vault write opportunistically
/// (throttled by the same backoff as the settings-load migration).
pub fn provider_api_key(app: &AppHandle, provider_id: &str) -> Option<String> {
    // `cli_agent/*` providers authenticate through the CLI's own session —
    // they can never hold a vault key, so skip the read entirely instead of
    // warning on the intentionally-invalid vault id every call.
    if crate::llm::cli_agent::is_cli_agent(provider_id) {
        return None;
    }
    match secret_store().get(provider_id) {
        Ok(Some(key)) if !key.is_empty() => return Some(key),
        Ok(_) => {}
        Err(e) => {
            warn!("Could not read '{provider_id}' key from the OS vault: {e}");
            // Vault state unknown — serve the pending plaintext but skip the
            // heal write: a blind `set` could overwrite a key we couldn't see.
            return legacy_pending_api_key(app, provider_id);
        }
    }

    let pending = legacy_pending_api_key(app, provider_id)?;
    // Opportunistic heal: retry the vault write and drop the plaintext copy
    // as soon as it succeeds instead of waiting for the next settings load.
    // Skipped under the in-memory backend — "healing" into a volatile store
    // and deleting the plaintext would lose the key on restart.
    if !memory_store_active() && vault_write_retry_allowed() {
        let healed = secret_store().set(provider_id, &pending).is_ok();
        if healed {
            if let Err(e) = remove_pending_api_key(app, provider_id) {
                warn!("'{provider_id}' is now vaulted but removing its plaintext copy failed: {e}");
            }
            debug!("Moved '{provider_id}' API key into the OS vault on read");
        }
        vault_write_retry_record(healed);
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
        // Separator classes also cover `\` so JSON-escaped forms like
        // `"x-api-key\":\"…"` / `"api_key\":\"…"` are caught too.
        r#"(?i)x-api-key["'\\\s]*[:=]["'\\\s]*[A-Za-z0-9._~+/=-]{8,}"#,
        // generic `api_key=` / `apikey: ` style parameters
        r#"(?i)api[-_]?key["'\\\s]*[:=]["'\\\s]*[A-Za-z0-9._~+/=-]{8,}"#,
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
        // `replace_all` borrows when nothing matched — only swap in the
        // owned copy on an actual rewrite so clean lines never allocate.
        if let Cow::Owned(replaced) = re.replace_all(&out, "<redacted>") {
            out = Cow::Owned(replaced);
        }
    }
    debug_assert_eq!(
        redaction_regexes().len(),
        REDACTION_PATTERN_COUNT,
        "a redaction pattern failed to compile — secrets could leak to logs"
    );
    out.into_owned()
}

#[cfg(test)]
mod tests;
