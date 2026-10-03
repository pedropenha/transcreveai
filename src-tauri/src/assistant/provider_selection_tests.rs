use super::state::choose_provider;
use crate::settings::AppSettings;

#[test]
fn auto_never_uses_experimental_fallback() {
    for experimental in ["cli_agent/cursor_agent", "cli_agent/devin"] {
        let mut settings = AppSettings {
            post_process_provider_id: experimental.to_string(),
            ..Default::default()
        };
        // Only the experimental provider is detected: Auto must still return
        // a stable, unavailable provider for its installation hint.
        let selected = choose_provider(&settings, |_| false, |id| id == experimental);
        assert_ne!(selected.map(|p| p.id.as_str()), Some(experimental));

        settings.assistant_provider_id = Some(experimental.to_string());
        let selected = choose_provider(&settings, |_| false, |id| id == experimental);
        assert_eq!(selected.map(|p| p.id.as_str()), Some(experimental));
    }
}
