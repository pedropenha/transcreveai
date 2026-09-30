//! Tray menu internationalization
//!
//! Everything is auto-generated at compile time by build.rs from the
//! frontend locale files (src/i18n/locales/*/translation.json).
//!
//! The English translation.json is the single source of truth:
//! - TrayStrings struct fields are derived from the English "tray" keys
//! - All languages are auto-discovered from the locales directory
//!
//! To add a new tray menu item:
//! 1. Add the key to en/translation.json under "tray"
//! 2. Add translations to other locale files
//! 3. Update tray.rs to use the new field (e.g., strings.new_field)

use once_cell::sync::Lazy;
use std::collections::HashMap;

// Include the auto-generated TrayStrings struct and TRANSLATIONS static
include!(concat!(env!("OUT_DIR"), "/tray_translations.rs"));

/// Get localized tray menu strings based on the system locale.
///
/// Lookup order: exact locale → language code → same primary subtag
/// ("pt" → "pt-BR") → English. Only `en` and `pt-BR` ship (ADR-0002).
pub fn get_tray_translations(locale: Option<String>) -> TrayStrings {
    let normalized = locale
        .as_deref()
        .unwrap_or("en")
        .to_lowercase()
        .replace('_', "-");
    let language = normalized.split('-').next().unwrap_or("en");

    TRANSLATIONS
        .iter()
        .find(|(code, _)| code.eq_ignore_ascii_case(&normalized))
        .map(|(_, strings)| strings)
        .or_else(|| TRANSLATIONS.get(language))
        .or_else(|| {
            TRANSLATIONS
                .iter()
                .find(|(code, _)| code.split('-').next() == Some(language))
                .map(|(_, strings)| strings)
        })
        .or_else(|| TRANSLATIONS.get("en"))
        .cloned()
        .expect("English translations must exist")
}

#[cfg(test)]
mod tests {
    use super::{get_tray_translations, TRANSLATIONS};

    #[test]
    fn resolves_locale_fallbacks() {
        for (locale, expected) in [
            ("en", "en"),
            ("en-US", "en"),
            ("pt-BR", "pt-BR"),
            ("pt_br", "pt-BR"),
            ("PT-BR", "pt-BR"),
            // Bare or non-Brazilian Portuguese folds onto pt-BR.
            ("pt", "pt-BR"),
            ("pt-PT", "pt-BR"),
            // Unsupported languages fall back to English.
            ("de-DE", "en"),
            ("xx-YY", "en"),
        ] {
            assert_eq!(
                format!("{:?}", get_tray_translations(Some(locale.into()))),
                format!("{:?}", TRANSLATIONS[expected]),
                "{locale} should resolve to {expected}"
            );
        }

        assert_eq!(
            format!("{:?}", get_tray_translations(None)),
            format!("{:?}", TRANSLATIONS["en"]),
            "missing locale should resolve to en"
        );
    }
}
