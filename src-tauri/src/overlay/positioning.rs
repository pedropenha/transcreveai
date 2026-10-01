//! Where the overlay window goes on screen: monitor selection (the one under
//! the cursor), cursor-vs-monitor hit testing, and the logical-coordinate
//! docked-position path used on macOS/Linux. The Windows physical-pixel path
//! lives in `win32.rs`; both share `geometry::docked_position`.

use tauri::{AppHandle, PhysicalPosition, PhysicalSize};

use super::geometry::{docked_position, edge_margin, effective_edge, OVERLAY_TOP_OFFSET};
use crate::input;
use crate::settings::{self, FlowbarFollow};

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

pub(crate) fn preferred_monitor_point(
    follow: FlowbarFollow,
    foreground: Option<(i32, i32)>,
) -> Option<(i32, i32)> {
    match follow {
        FlowbarFollow::ForegroundMonitor => foreground,
        FlowbarFollow::Cursor | FlowbarFollow::PrimaryMonitor => None,
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn monitor_index_for_rect(
    rect: (f64, f64, f64, f64),
    monitors: &[(i32, i32, u32, u32)],
) -> Option<usize> {
    let center = (
        (rect.0 + rect.2 / 2.0).round() as i32,
        (rect.1 + rect.3 / 2.0).round() as i32,
    );
    monitors.iter().position(|(x, y, width, height)| {
        center.0 >= *x
            && center.0 < *x + *width as i32
            && center.1 >= *y
            && center.1 < *y + *height as i32
    })
}

#[cfg(target_os = "windows")]
fn foreground_window_center() -> Option<(i32, i32)> {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowRect};

    // SAFETY: GetForegroundWindow has no memory-safety preconditions; the
    // returned handle is only passed to GetWindowRect while `rect` is a valid,
    // initialized out-buffer owned by this call.
    let window = unsafe { GetForegroundWindow() };
    if window.0.is_null() {
        return None;
    }
    let mut rect = RECT::default();
    unsafe { GetWindowRect(window, &mut rect) }.ok()?;
    Some(((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2))
}

#[cfg(not(target_os = "windows"))]
fn foreground_window_center() -> Option<(i32, i32)> {
    None
}

fn monitor_containing_physical_point(
    app_handle: &AppHandle,
    point: (i32, i32),
) -> Option<tauri::Monitor> {
    app_handle
        .available_monitors()
        .ok()?
        .into_iter()
        .find(|monitor| is_mouse_within_monitor(point, monitor.position(), monitor.size()))
}

pub(crate) fn get_flowbar_monitor(app_handle: &AppHandle) -> Option<tauri::Monitor> {
    let follow = settings::get_settings(app_handle).flowbar_follow;
    if follow == FlowbarFollow::Cursor {
        return get_monitor_with_cursor(app_handle);
    }
    let foreground = foreground_window_center();
    #[cfg(not(target_os = "windows"))]
    if follow == FlowbarFollow::ForegroundMonitor && foreground.is_none() {
        return get_monitor_with_cursor(app_handle);
    }
    preferred_monitor_point(follow, foreground)
        .and_then(|point| monitor_containing_physical_point(app_handle, point))
        .or_else(|| app_handle.primary_monitor().ok().flatten())
}

#[cfg(target_os = "windows")]
pub(crate) fn get_monitor_for_rect(
    app_handle: &AppHandle,
    rect: (f64, f64, f64, f64),
) -> Option<tauri::Monitor> {
    let monitors = app_handle.available_monitors().ok()?;
    let areas = monitors
        .iter()
        .map(|monitor| {
            let position = monitor.position();
            let size = monitor.size();
            (position.x, position.y, size.width, size.height)
        })
        .collect::<Vec<_>>();
    monitor_index_for_rect(rect, &areas).and_then(|index| monitors.get(index).cloned())
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
    let monitor = get_flowbar_monitor(app_handle)?;
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
