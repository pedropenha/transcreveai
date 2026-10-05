//! Session-scoped local translation policy. Never modifies stored preferences.

/// Reject unavailable or incompatible models before opening the microphone and
/// again before inference. Unknown capabilities fail closed.
pub(crate) fn validate_translation_model(
    exists: bool,
    local_cpp: bool,
    supports_translation: bool,
    downloaded: bool,
) -> Result<(), &'static str> {
    if !exists {
        return Err("translation_model_required");
    }
    if !local_cpp || !supports_translation {
        return Err("translation_model_incompatible");
    }
    if !downloaded {
        return Err("translation_model_unavailable");
    }
    Ok(())
}

/// Build an isolated configuration for one translated session.
pub(crate) fn translation_settings(
    settings: &crate::settings::AppSettings,
) -> crate::settings::AppSettings {
    let mut snapshot = settings.clone();
    snapshot.selected_model = settings
        .translation_model_id
        .clone()
        .unwrap_or_else(|| settings.selected_model.clone());
    // The translated command detects its own source speech; the ordinary
    // language preference (including English-only) remains unchanged.
    snapshot.selected_language = "auto".to_string();
    snapshot.translate_to_english = true;
    snapshot.dictation_provider_id = None;
    snapshot.fallback_provider_id = None;
    snapshot
}

/// Fail closed after loading and after leasing the actual engine.
pub(crate) fn validate_loaded_translation(
    expected: &str,
    actual: Option<&str>,
    compatible_engine: bool,
) -> Result<(), &'static str> {
    if actual != Some(expected) {
        return Err("translation_model_changed");
    }
    if !compatible_engine {
        return Err("translation_model_incompatible");
    }
    Ok(())
}

/// No translated output may reach a different target or a cancelled session.
pub(crate) fn translation_delivery_allowed(
    cancelled: bool,
    probed: bool,
    has_window: bool,
    original: Option<usize>,
    current: Option<usize>,
) -> bool {
    !cancelled && (!probed || (has_window && original.is_some() && original == current))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translation_snapshot_does_not_change_preferences_or_fallback() {
        let mut stored = crate::settings::get_default_settings();
        stored.selected_model = "turbo".into();
        stored.translation_model_id = Some("medium".into());
        stored.translate_to_english = false;
        stored.selected_language = "pt".into();
        stored.dictation_provider_id = Some("ordinary".into());
        stored.fallback_provider_id = Some("fallback".into());
        let snapshot = translation_settings(&stored);
        assert_eq!(snapshot.selected_model, "medium");
        assert!(snapshot.translate_to_english);
        assert_eq!(snapshot.selected_language, "auto");
        assert_eq!(stored.selected_language, "pt");
        assert_eq!(snapshot.dictation_provider_id, None);
        assert_eq!(snapshot.fallback_provider_id, None);
        assert_eq!(stored.selected_model, "turbo");
        assert!(!stored.translate_to_english);
        assert_eq!(stored.fallback_provider_id.as_deref(), Some("fallback"));
        stored.translation_model_id = None;
        assert_eq!(translation_settings(&stored).selected_model, "turbo");
        stored.selected_language = "en".into();
        assert_eq!(translation_settings(&stored).selected_language, "auto");
        assert_eq!(stored.selected_language, "en");
    }

    #[test]
    fn translation_loaded_model_must_match_snapshot_and_capability() {
        assert_eq!(
            validate_loaded_translation("medium", Some("medium"), true),
            Ok(())
        );
        assert_eq!(
            validate_loaded_translation("medium", Some("parakeet"), false),
            Err("translation_model_changed")
        );
        assert_eq!(
            validate_loaded_translation("medium", None, false),
            Err("translation_model_changed")
        );
        assert_eq!(
            validate_loaded_translation("medium", Some("medium"), false),
            Err("translation_model_incompatible")
        );
    }

    #[test]
    fn translation_delivery_preserves_destination_and_cancel() {
        assert!(translation_delivery_allowed(
            false,
            true,
            true,
            Some(1),
            Some(1)
        ));
        assert!(!translation_delivery_allowed(
            false,
            true,
            true,
            Some(1),
            Some(2)
        ));
        assert!(!translation_delivery_allowed(
            false, true, false, None, None
        ));
        assert!(!translation_delivery_allowed(
            true,
            true,
            true,
            Some(1),
            Some(1)
        ));
        assert!(translation_delivery_allowed(
            false, false, false, None, None
        ));
        assert!(!translation_delivery_allowed(
            true, false, false, None, None
        ));
    }

    #[test]
    fn translation_accepts_ready_local_models() {
        assert_eq!(validate_translation_model(true, true, true, true), Ok(()));
    }

    #[test]
    fn translation_rejects_unknown_model() {
        assert_eq!(
            validate_translation_model(false, true, true, true),
            Err("translation_model_required")
        );
    }

    #[test]
    fn translation_rejects_turbo_and_nonlocal_models() {
        assert_eq!(
            validate_translation_model(true, true, false, true),
            Err("translation_model_incompatible")
        );
        assert_eq!(
            validate_translation_model(true, false, true, true),
            Err("translation_model_incompatible")
        );
    }

    #[test]
    fn translation_requires_explicit_download() {
        assert_eq!(
            validate_translation_model(true, true, true, false),
            Err("translation_model_unavailable")
        );
    }
}
