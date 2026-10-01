//! Windows-only window management for the Flow Bar overlay: the
//! `SetWindowPos`-based placement path that bypasses tao's current-DPI logical
//! conversion (which mislands cross-monitor moves), the forced-topmost
//! reassertion, and the extended styles that keep the bar non-focusable
//! (NFR-001-01). All coordinates are physical pixels of the destination
//! monitor's work area.

use std::sync::atomic::AtomicBool;
use tauri::AppHandle;

use super::geometry::{
    docked_position, effective_edge, BarEdge, FLOWBAR_EDGE_MARGIN, OVERLAY_TOP_OFFSET,
};
use super::positioning::get_monitor_with_cursor;
use crate::settings;

/// Whether the currently shown overlay state is the streaming panel — the
/// reposition path (`update_overlay_position_on_main`) reads this to keep the
/// right footprint. Set in `show_overlay_state_on_main`.
pub(crate) static WINDOWS_OVERLAY_IS_STREAMING: AtomicBool = AtomicBool::new(false);

/// Windows accessibility text size (Settings > Accessibility > Text size), a
/// separate axis from display scaling that WebView2 applies as a document zoom.
pub(crate) fn windows_text_scale_factor() -> f64 {
    // Absent until the user moves the slider off 100%; stored as a percentage.
    winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Accessibility")
        .and_then(|key| key.get_value::<u32, _>("TextScaleFactor"))
        .map(|percent| (percent as f64 / 100.0).clamp(1.0, 2.25))
        .unwrap_or(1.0)
}

/// Forces a window to be topmost using Win32 API.
/// This is more reliable than Tauri's set_always_on_top which can be overridden.
pub(crate) fn force_overlay_topmost(overlay_window: &tauri::webview::WebviewWindow) {
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW,
    };

    // Clone because run_on_main_thread takes 'static
    let overlay_clone = overlay_window.clone();

    // Make sure the Win32 call happens on the UI thread
    let _ = overlay_clone.clone().run_on_main_thread(move || {
        if let Ok(hwnd) = overlay_clone.hwnd() {
            unsafe {
                // Force Z-order: make this window topmost without changing size/pos or stealing focus
                let _ = SetWindowPos(
                    hwnd,
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
            }
        }
    });
}

/// NFR-001-01: the Flow Bar must never receive focus — clicking it cannot
/// change the foreground window (AC-001-01). tao's `focusable(false)` already
/// maps to WS_EX_NOACTIVATE; spell the full style set out anyway since the
/// F001 technical notes mandate WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW |
/// WS_EX_TOPMOST explicitly.
pub(crate) fn apply_overlay_extended_styles(overlay_window: &tauri::webview::WebviewWindow) {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, WINDOW_EX_STYLE, WS_EX_NOACTIVATE,
        WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    };

    let window = overlay_window.clone();
    let _ = window.clone().run_on_main_thread(move || {
        if let Ok(hwnd) = window.hwnd() {
            unsafe {
                let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
                let extended =
                    WINDOW_EX_STYLE(style) | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST;
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, extended.0 as isize);
            }
        }
    });
}

/// Overlay rectangle in the destination monitor's physical pixels, so nothing
/// is converted through the window's previous-monitor DPI. Docking uses the
/// monitor's *work area* (FR-001-01: bottom-center, above the taskbar).
fn windows_overlay_bounds(
    monitor: &tauri::Monitor,
    scale: f64,
    text_scale: f64,
    logical_width: f64,
    logical_height: f64,
    edge: BarEdge,
    offset: f64,
) -> (i32, i32, i32, i32) {
    let wa = monitor.work_area();
    windows_overlay_bounds_from_area(
        (wa.position.x, wa.position.y, wa.size.width, wa.size.height),
        scale,
        text_scale,
        logical_width,
        logical_height,
        edge,
        offset,
    )
}

/// Pure half of `windows_overlay_bounds`: a physical-pixel work-area rect in
/// plus DPI/text scales plus logical window size → physical window bounds.
/// Split out so tests can exercise it without constructing a `tauri::Monitor`.
/// `pub(crate)` for `overlay::tests`.
pub(crate) fn windows_overlay_bounds_from_area(
    work_area: (i32, i32, u32, u32),
    scale: f64,
    text_scale: f64,
    logical_width: f64,
    logical_height: f64,
    edge: BarEdge,
    offset: f64,
) -> (i32, i32, i32, i32) {
    // Grow the window with the text scale; offsets stay DPI-only since the
    // card sits flush against the window's screen-edge side.
    let content_scale = scale * text_scale;
    let width = (logical_width * content_scale).round().max(1.0);
    let height = (logical_height * content_scale).round().max(1.0);

    let area = (
        work_area.0 as f64,
        work_area.1 as f64,
        work_area.2 as f64,
        work_area.3 as f64,
    );
    let (x, y) = docked_position(
        area,
        (width, height),
        edge,
        offset,
        OVERLAY_TOP_OFFSET * scale,
        FLOWBAR_EDGE_MARGIN * scale,
    );

    (
        x.round() as i32,
        y.round() as i32,
        width as i32,
        height as i32,
    )
}

/// Generic physical-pixel move+size shared with the toast window (T-062):
/// one `SetWindowPos`, no activation, no Z-order change. Bypasses tao's
/// current-DPI logical conversion, which mislands cross-monitor moves.
pub(crate) fn set_window_bounds_physical(
    window: &tauri::webview::WebviewWindow,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) -> Result<(), String> {
    use windows::Win32::UI::WindowsAndMessaging::{SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER};

    let hwnd = window
        .hwnd()
        .map_err(|error| format!("failed to get window handle: {error}"))?;

    // SAFETY: `hwnd` is a valid window handle owned by this process (obtained
    // from tao); the remaining arguments are plain coordinates/flags — no
    // borrowed buffers or out-params are involved.
    unsafe {
        SetWindowPos(
            hwnd,
            None,
            x,
            y,
            width,
            height,
            SWP_NOACTIVATE | SWP_NOZORDER,
        )
        .map_err(|error| format!("failed to set window bounds: {error}"))?;
    }
    Ok(())
}

/// Moves and sizes the overlay in one native SetWindowPos, bypassing tao's
/// current-DPI logical conversion that mislands cross-monitor moves.
pub(crate) fn place_windows_overlay(
    app_handle: &AppHandle,
    overlay_window: &tauri::webview::WebviewWindow,
    logical_width: f64,
    logical_height: f64,
) -> Result<(), String> {
    let monitor = get_monitor_with_cursor(app_handle)
        .ok_or_else(|| "failed to determine the monitor containing the cursor".to_string())?;
    let text_scale = windows_text_scale_factor();
    let settings = settings::get_settings(app_handle);
    let (x, y, width, height) = windows_overlay_bounds(
        &monitor,
        monitor.scale_factor(),
        text_scale,
        logical_width,
        logical_height,
        effective_edge(&settings),
        settings.flowbar_position_offset,
    );
    set_window_bounds_physical(overlay_window, x, y, width, height)
        .map_err(|error| format!("failed to set overlay bounds: {error}"))?;

    log::debug!(
        "windows overlay bounds: x={} y={} width={} height={} scale={} text_scale={}",
        x,
        y,
        width,
        height,
        monitor.scale_factor(),
        text_scale
    );
    Ok(())
}
