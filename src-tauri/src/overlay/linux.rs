//! Linux GTK layer-shell wiring for the Flow Bar overlay — the compositor
//! anchors the surface to a screen edge and keeps it above everything without
//! window-manager placement. `LAYER_SHELL_ACTIVE` records whether the
//! compositor spoke layer-shell at init; every later call site consults it so
//! the plain-window fallback still works.

use std::sync::atomic::{AtomicBool, Ordering};

use gtk_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use tauri::Manager;

use super::geometry::{effective_edge, BarEdge, FLOWBAR_EDGE_MARGIN, OVERLAY_TOP_OFFSET};
use crate::settings;
use crate::utils;

/// Whether gtk-layer-shell was successfully initialized. Used to skip
/// layer-shell calls when the window is a regular fallback.
pub(crate) static LAYER_SHELL_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Configures the edge and offset of a GTK layer surface. gtk-layer-shell
/// commits anchor and margin changes itself, including while the surface is
/// mapped, so changing position does not require a manual hide/show cycle.
/// Anchoring a single edge centers the surface along it — the relative offset
/// along the edge lands with T-041's drag/dock work.
pub(crate) fn configure_layer_shell_position(gtk_window: &gtk::ApplicationWindow, edge: BarEdge) {
    let anchor = match edge {
        BarEdge::Top => Edge::Top,
        BarEdge::Bottom => Edge::Bottom,
        BarEdge::Left => Edge::Left,
        BarEdge::Right => Edge::Right,
    };
    let margin = match edge {
        BarEdge::Top => OVERLAY_TOP_OFFSET,
        _ => FLOWBAR_EDGE_MARGIN,
    };

    for e in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
        gtk_window.set_anchor(e, e == anchor);
        gtk_window.set_layer_shell_margin(
            e,
            if e == anchor {
                margin.round() as i32
            } else {
                0
            },
        );
    }
}

/// Configures a GTK layer surface before it is shown.
///
/// Tauri's normal `set_size` path calls `gtk_window_resize`, but layer surfaces
/// derive their dimensions from GTK's size request. gtk-layer-shell documents
/// the `set_size_request` + `resize(1, 1)` sequence for forcing a new size.
pub(crate) fn configure_layer_shell_surface(
    gtk_window: &gtk::ApplicationWindow,
    edge: BarEdge,
    width: f64,
    height: f64,
) {
    use gtk::prelude::{GtkWindowExt, WidgetExt};

    configure_layer_shell_position(gtk_window, edge);

    gtk_window.set_size_request(
        width.round().max(1.0) as i32,
        height.round().max(1.0) as i32,
    );
    gtk_window.resize(1, 1);
}

/// Initializes GTK layer shell for the Linux overlay window.
/// Returns true if layer shell was successfully initialized, false otherwise.
pub(crate) fn init_gtk_layer_shell(
    overlay_window: &tauri::webview::WebviewWindow,
    width: f64,
    height: f64,
) -> bool {
    if utils::env_flag_enabled("TRANSCREVE_NO_GTK_LAYER_SHELL") {
        log::debug!("Skipping GTK layer shell init (TRANSCREVE_NO_GTK_LAYER_SHELL is enabled)");
        return false;
    }

    if !gtk_layer_shell::is_supported() {
        return false;
    }

    // Try to get the GTK window from the Tauri webview
    if let Ok(gtk_window) = overlay_window.gtk_window() {
        gtk_window.init_layer_shell();
        gtk_window.set_layer(Layer::Overlay);
        gtk_window.set_keyboard_mode(KeyboardMode::None);
        gtk_window.set_exclusive_zone(0);

        let settings = settings::get_settings(overlay_window.app_handle());
        configure_layer_shell_surface(&gtk_window, effective_edge(&settings), width, height);

        let initialized = gtk_window.is_layer_window();
        LAYER_SHELL_ACTIVE.store(initialized, Ordering::SeqCst);
        return initialized;
    }
    false
}
