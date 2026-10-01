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
fn hint_reveals_at_most_a_quarter_of_the_key() {
    assert_eq!(hint_for(None), None);
    assert_eq!(hint_for(Some("")), None);
    // ≤3 chars: nothing is revealed.
    assert_eq!(hint_for(Some("abc")), Some("••••".to_string()));
    // len/4 revealed, capped at 4: never more than 25% of the key.
    assert_eq!(hint_for(Some("abcd")), Some("••••d".to_string()));
    assert_eq!(hint_for(Some("abcde")), Some("••••e".to_string()));
    assert_eq!(hint_for(Some("sk-proj-xyzw")), Some("••••yzw".to_string()));
    let hint = hint_for(Some("sk-proj-123456789")).unwrap();
    assert_eq!(hint, "••••6789");
    assert!(!hint.contains("sk-"), "hint must not leak key material");
}

#[test]
fn memory_store_debug_never_leaks_key_material() {
    let store = MemorySecretStore::default();
    store.set("openai", "sk-secret-material").unwrap();
    let rendered = format!("{store:?}");
    assert!(rendered.contains("MemorySecretStore"));
    assert!(
        !rendered.contains("sk-secret-material"),
        "Debug output must not expose stored secrets: {rendered}"
    );
}

#[test]
fn validate_secret_rejects_control_chars_and_oversized_input() {
    assert!(validate_secret("sk-ok").is_ok());
    assert!(validate_secret("bad\nkey").is_err());
    assert!(validate_secret("bad\r\nkey").is_err());
    assert!(validate_secret("bad\tkey").is_err());
    assert!(validate_secret("bad\u{0}key").is_err());
    assert!(validate_secret(&"x".repeat(MAX_SECRET_BYTES)).is_ok());
    assert!(validate_secret(&"x".repeat(MAX_SECRET_BYTES + 1)).is_err());
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

/// Store whose reads always fail but whose writes are recorded — stands
/// in for a vault whose state cannot be inspected before migration.
struct UnreadableSecretStore {
    sets: Mutex<Vec<(String, String)>>,
}

impl SecretStore for UnreadableSecretStore {
    fn get(&self, _provider_id: &str) -> Result<Option<String>, SecretError> {
        Err(SecretError("store unreadable".to_string()))
    }
    fn set(&self, provider_id: &str, secret: &str) -> Result<(), SecretError> {
        self.sets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((provider_id.to_string(), secret.to_string()));
        Ok(())
    }
    fn delete(&self, _provider_id: &str) -> Result<(), SecretError> {
        Ok(())
    }
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

/// Regression: a pending plaintext key stranded by an earlier vault
/// failure must never overwrite a newer key the user saved since — the
/// vault wins and the stale copy is dropped.
#[test]
fn migration_never_overwrites_a_newer_vaulted_key() {
    let store = MemorySecretStore::default();
    store.set("openai", "sk-new-k2").unwrap();
    let mut json = json!({
        "post_process_api_keys": {"openai": "sk-stale-k1"},
    });
    assert!(migrate_plaintext_api_keys(&mut json, &store));
    assert!(json.get("post_process_api_keys").is_none());
    assert_eq!(
        store.get("openai").unwrap().as_deref(),
        Some("sk-new-k2"),
        "the vaulted key must win over the stale plaintext copy"
    );
}

/// When the vault cannot even be read, migration must not blind-write —
/// the key stays pending for a later attempt.
#[test]
fn migration_never_blind_writes_to_an_unreadable_vault() {
    let store = UnreadableSecretStore {
        sets: Mutex::new(Vec::new()),
    };
    let mut json = json!({
        "post_process_api_keys": {"openai": "sk-pending"},
    });
    assert!(!migrate_plaintext_api_keys(&mut json, &store));
    assert_eq!(
        json["post_process_api_keys"]["openai"].as_str(),
        Some("sk-pending")
    );
    assert!(
        store
            .sets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty(),
        "migration must not write to a vault whose state it cannot read"
    );
}

#[test]
fn migration_trims_legacy_keys_before_storing() {
    let store = MemorySecretStore::default();
    let mut json = json!({
        "post_process_api_keys": {"openai": "  sk-padded \n"},
    });
    assert!(migrate_plaintext_api_keys(&mut json, &store));
    assert_eq!(store.get("openai").unwrap().as_deref(), Some("sk-padded"));
}

#[test]
fn migration_drops_non_string_leftovers() {
    let store = MemorySecretStore::default();
    let mut json = json!({
        "post_process_api_keys": {"weird": 42, "openai": "sk-x1"},
    });
    assert!(migrate_plaintext_api_keys(&mut json, &store));
    // Non-string entries were never usable keys — they are dropped with
    // the field instead of pending migration forever.
    assert!(json.get("post_process_api_keys").is_none());
    assert_eq!(store.get("openai").unwrap().as_deref(), Some("sk-x1"));
}

#[test]
fn vault_write_retry_backoff_throttles_failures() {
    // Reset the shared schedule first so the test is order-independent.
    vault_write_retry_record(true);
    assert!(vault_write_retry_allowed(), "first attempt is allowed");
    vault_write_retry_record(false);
    assert!(
        !vault_write_retry_allowed(),
        "a failed attempt defers the next one"
    );
    vault_write_retry_record(true);
    assert!(
        vault_write_retry_allowed(),
        "a success resets the backoff schedule"
    );
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
        // JSON-escaped form, as it appears when a payload is embedded in a
        // serialized request/response inside a log line.
        "post body {\"api_key\":\"ZXCVBNM1234567\"}",
    ];
    for case in cases {
        let redacted = redact_sensitive(case);
        assert!(redacted.contains("<redacted>"), "{case}");
        assert!(
            !redacted.contains("abc") && !redacted.contains("ZXCVBNM"),
            "secret material must not survive: {redacted}"
        );
    }
}

#[test]
fn redaction_leaves_normal_text_alone() {
    let msg = "Loading model whisper-base for provider openai";
    assert_eq!(redact_sensitive(msg), msg);
}
