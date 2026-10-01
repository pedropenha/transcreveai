//! Where the overlay window goes on screen: monitor selection (the one under
//! the cursor), cursor-vs-monitor hit testing, and the logical-coordinate
//! docked-position path used on macOS/Linux. The Windows physical-pixel path
//! lives in `win32.rs`; both share `geometry::docked_position`.

use tauri::{AppHandle, PhysicalPosition, PhysicalSize};

use super::geometry::{docked_position, edge_margin, effective_edge, OVERLAY_TOP_OFFSET};
use crate::input;
use crate::settings;

pub(crate) fn get_monitor_with_cursor(app_handle: &AppHandle) -> Option<tauri::Monitor> {
    if let Some(mouse_location) = input::get_cursor_position(app_handle) {
        if let Ok(monitors) = app_handle.available_monitors() {
            for monitor in monitors {
                // On Windows both the cursor (enigo -> GetCursorPos) and the
                // monitor bounds are physical pixels, so compare them directly.
                #[cfg(target_os = "windows")]
                if is_mouse_within_monitor(mouse_location, monitor.position(), monitor.size()) {
                    return Some(monitor);
                }

                // macOS/Linux: enigo returns logical coords, so scale the bounds down.
                #[cfg(not(target_os = "windows"))]
                {
                    let scale = monitor.scale_factor();
                    let pos = PhysicalPosition::new(
                        (monitor.position().x as f64 / scale) as i32,
                        (monitor.position().y as f64 / scale) as i32,
                    );
                    let size = PhysicalSize::new(
                        (monitor.size().width as f64 / scale) as u32,
                        (monitor.size().height as f64 / scale) as u32,
                    );
                    if is_mouse_within_monitor(mouse_location, &pos, &size) {
                        return Some(monitor);
                    }
                }
            }
        }
    }

    app_handle.primary_monitor().ok().flatten()
}

pub(crate) fn is_mouse_within_monitor(
    mouse_pos: (i32, i32),
    monitor_pos: &PhysicalPosition<i32>,
    monitor_size: &PhysicalSize<u32>,
) -> bool {
    let (mouse_x, mouse_y) = mouse_pos;
    let PhysicalPosition {
        x: monitor_x,
        y: monitor_y,
    } = *monitor_pos;
    let PhysicalSize {
        width: monitor_width,
        height: monitor_height,
    } = *monitor_size;

    mouse_x >= monitor_x
        && mouse_x < (monitor_x + monitor_width as i32)
        && mouse_y >= monitor_y
        && mouse_y < (monitor_y + monitor_height as i32)
}

/// Returns overlay position in logical coordinates (points on macOS).
///
/// The docked edge comes from the Flow Bar settings (bottom-center inside the
/// work area by default, FR-001-01); the legacy `overlay_position` top/bottom
/// is honored through `effective_edge`. macOS uses the work area
/// (visibleFrame) so the bar tracks the Dock; Linux keeps full monitor bounds
/// (work_area is unreliable on Wayland).
///
/// We must use LogicalPosition (not PhysicalPosition) because Tauri/tao
/// converts PhysicalPosition using the scale factor of the monitor the window
/// is *currently* on, which is wrong when moving cross-monitor. Windows uses
/// `place_windows_overlay` instead (no single logical space across mixed DPI).
pub(crate) fn calculate_overlay_position(
    app_handle: &AppHandle,
    width: f64,
    height: f64,
) -> Option<(f64, f64)> {
    let monitor = get_monitor_with_cursor(app_handle)?;
    let scale = monitor.scale_factor();
    let settings = settings::get_settings(app_handle);

    // `area` is the rect the bar docks against, in logical units.
    // work_area.position shares monitor.position's global coordinate space.
    let area = {
        #[cfg(target_os = "macos")]
        {
            let wa = monitor.work_area();
            (
                wa.position.x as f64 / scale,
                wa.position.y as f64 / scale,
                wa.size.width as f64 / scale,
                wa.size.height as f64 / scale,
            )
        }
        #[cfg(not(target_os = "macos"))]
        (
            monitor.position().x as f64 / scale,
            monitor.position().y as f64 / scale,
            monitor.size().width as f64 / scale,
            monitor.size().height as f64 / scale,
        )
    };

    Some(docked_position(
        area,
        (width, height),
        effective_edge(&settings),
        settings.flowbar_position_offset,
        OVERLAY_TOP_OFFSET,
        edge_margin(),
    ))
}

/// Current overlay window size in logical units (points), for repositioning
/// without assuming a fixed size (Flow Bar vs. streaming).
#[cfg(not(target_os = "windows"))]
pub(crate) fn current_overlay_logical_size(
    window: &tauri::webview::WebviewWindow,
) -> Option<(f64, f64)> {
    let size = window.inner_size().ok()?;
    let scale = window.scale_factor().ok()?;
    Some((size.width as f64 / scale, size.height as f64 / scale))
}
