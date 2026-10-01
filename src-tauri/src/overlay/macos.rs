//! macOS Flow Bar overlay: the overlay is an `NSPanel` (non-activating, can
//! join all Spaces, floating above full-screen apps) built through
//! tauri-nspanel. The window stays registered under the `flowbar` label so the
//! shared emit/geometry paths keep working.

use tauri::{AppHandle, Listener, Manager, WebviewUrl};
use tauri_nspanel::{tauri_panel, CollectionBehavior, PanelBuilder, PanelLevel, StyleMask};

use super::positioning::calculate_overlay_position;

tauri_panel! {
    panel!(RecordingOverlayPanel {
        config: {
            can_become_key_window: false,
            is_floating_panel: true
        }
    })
}

/// Creates the recording overlay panel and keeps it hidden by default (macOS)
pub fn create_recording_overlay(app_handle: &AppHandle) {
    let (width, height) = (super::FLOWBAR_WIDTH, super::FLOWBAR_HEIGHT);
    if let Some((x, y)) = calculate_overlay_position(app_handle, width, height) {
        // PanelBuilder creates a Tauri window then converts it to NSPanel.
        // The window remains registered, so get_webview_window() still works.
        match PanelBuilder::<_, RecordingOverlayPanel>::new(
            app_handle,
            crate::window_labels::FLOWBAR,
        )
        .url(WebviewUrl::App("src/overlay/index.html".into()))
        .title("Recording")
        .position(tauri::Position::Logical(tauri::LogicalPosition { x, y }))
        .level(PanelLevel::Status)
        .size(tauri::Size::Logical(tauri::LogicalSize { width, height }))
        .has_shadow(false)
        .transparent(true)
        .no_activate(true)
        .corner_radius(0.0)
        .style_mask(StyleMask::empty().borderless().nonactivating_panel())
        .with_window(|w| w.decorations(false).transparent(true).focusable(false))
        .collection_behavior(
            CollectionBehavior::new()
                .can_join_all_spaces()
                .full_screen_auxiliary(),
        )
        .build()
        {
            Ok(panel) => {
                panel.hide();
                // NFR-001-02 click-through poll — the panel's WebviewWindow
                // stays registered under the label.
                if let Some(window) = app_handle.get_webview_window(crate::window_labels::FLOWBAR) {
                    super::click_through::start(&window);
                }
                app_handle.listen(
                    crate::transcription_coordinator::SESSION_STATE_EVENT,
                    |event| super::note_session_state(event.payload()),
                );
            }
            Err(e) => {
                log::error!("Failed to create recording overlay panel: {}", e);
            }
        }
    }
}
