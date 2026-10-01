//! Labels of the app's webview windows.
//!
//! Tauri grants IPC permissions per window label (`capabilities/*.json`), so a
//! label that drifts from the capability files silently loses access to every
//! command. Keep all labels here and reference these constants instead of
//! string literals.

/// Main settings/home window (the Wispr "Hub").
pub const HUB: &str = "hub";

/// Always-on-top recording indicator (the Wispr "Flow Bar").
pub const FLOWBAR: &str = "flowbar";

/// Non-activatable meeting-detection toast (F008, T-062).
pub const TOAST: &str = "toast";

/// The meeting window (F009, T-066) — a normal decorated window hosting
/// "Minhas notas" / Transcrição / Resumo. Closing it hides the window; the
/// session keeps recording (FR-009-14).
pub const MEETING: &str = "meeting";

/// The voice-assistant overlay panel (F012, T-091) — a topmost floating
/// panel that appears without stealing focus and becomes editable once the
/// user clicks it. Closing it hides the window; the conversation session
/// stays in memory only.
pub const ASSISTANT: &str = "assistant";

#[cfg(test)]
mod tests {
    use super::*;

    fn capability_windows(json: &str) -> Vec<String> {
        let value: serde_json::Value = serde_json::from_str(json).expect("valid capability JSON");
        value["windows"]
            .as_array()
            .expect("capability lists its windows")
            .iter()
            .map(|w| w.as_str().expect("window label is a string").to_owned())
            .collect()
    }

    #[test]
    fn default_capability_covers_all_webview_windows() {
        let windows = capability_windows(include_str!("../capabilities/default.json"));
        assert_eq!(
            windows,
            vec![
                HUB.to_owned(),
                FLOWBAR.to_owned(),
                TOAST.to_owned(),
                MEETING.to_owned(),
                ASSISTANT.to_owned()
            ]
        );
    }

    #[test]
    fn desktop_capability_covers_only_the_hub() {
        let windows = capability_windows(include_str!("../capabilities/desktop.json"));
        assert_eq!(windows, vec![HUB.to_owned()]);
    }
}
