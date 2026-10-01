//! NFR-001-02 click-through for the Flow Bar.
//!
//! The overlay window is transparent and much larger than the visible pill —
//! its transparent frame must pass clicks to the app below (AC-001-02) while
//! the pill itself keeps receiving hover/click (AC-001-03). Per the F001
//! technical notes the window runs with `set_ignore_cursor_events(true)` by
//! default; this module polls the cursor (≈30 Hz, only while it is within
//! 60 px of the window) and disables click-through while the cursor is inside
//! the interactive rectangle the webview reports via `flowbar_set_hover`.
//! `flowbar://cursor` events tell the webview when the flag flips, so a hover
//! that outlives its rect (window went click-through mid-hover) collapses.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

/// Interactive rectangle in CSS px, relative to the Flow Bar window's
/// top-left corner, reported by the webview through `flowbar_set_hover`
/// (contracts.md §5 — the "bounds" form; a bare `hovering` bool cannot work
/// because the webview never sees the cursor while click-through is on).
/// `None` means nothing on screen is interactive: pass everything through.
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, specta::Type)]
pub struct FlowbarRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

static INTERACTIVE_RECT: Mutex<Option<FlowbarRect>> = Mutex::new(None);
static LOOP_STARTED: AtomicBool = AtomicBool::new(false);
/// Last state pushed to the window — true while it ignores cursor events.
/// Starts false: before the first rect report the pill must stay clickable
/// (fail-open towards usability rather than a dead overlay).
static IGNORING: AtomicBool = AtomicBool::new(false);

/// Store the webview's interactive rect (`flowbar_set_hover` command).
pub fn set_interactive_rect(rect: Option<FlowbarRect>) {
    *INTERACTIVE_RECT.lock().unwrap_or_else(|e| e.into_inner()) = rect;
}

/// Poll cadence: fast while the cursor is near the window (≈30 Hz, inside the
/// FR-001-02 hover budget), lazy far away (NFR-001-04 — idle CPU ≈ 0).
const NEAR_POLL: Duration = Duration::from_millis(33);
const FAR_POLL: Duration = Duration::from_millis(80);
/// Long cadence for "nothing to do" states (window hidden / gone, no cursor
/// source on this platform).
const IDLE_POLL: Duration = Duration::from_millis(300);
/// Cursor distance from the window that switches the loop to the fast cadence
/// and enables the hover hit-test (spec: ≤ 60 px).
const NEAR_MARGIN: f64 = 60.0;
/// Cushion around the reported rect so a cursor straddling the pill edge does
/// not flap the click-through flag back and forth.
const RECT_CUSHION: f64 = 4.0;

/// Start the cursor hit-test loop. Idempotent — safe to call per overlay
/// (re)creation.
pub fn start(window: &tauri::webview::WebviewWindow) {
    if LOOP_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = window.app_handle().clone();
    let spawned = std::thread::Builder::new()
        .name("flowbar-hit-test".to_string())
        .spawn(move || {
            while app
                .get_webview_window(crate::window_labels::FLOWBAR)
                .is_some()
            {
                std::thread::sleep(tick(&app));
            }
            log::debug!("Flow Bar hit-test loop exiting: overlay window gone");
        });
    if let Err(e) = spawned {
        log::error!("Failed to spawn Flow Bar hit-test loop: {e}");
    }
}

/// One poll iteration; returns the cadence for the next tick.
fn tick(app: &AppHandle) -> Duration {
    let Some(window) = app.get_webview_window(crate::window_labels::FLOWBAR) else {
        return IDLE_POLL;
    };
    if !window.is_visible().unwrap_or(false) {
        return IDLE_POLL;
    }
    let Some(cursor) = cursor_position(app) else {
        // No cursor source (e.g. Enigo not initialised yet on macOS/Linux —
        // it is wired up by `initialize_enigo` after onboarding). Leave the
        // window interactive: a clickable overlay beats a dead one.
        return IDLE_POLL;
    };
    let (Ok(origin), Ok(size)) = (window.outer_position(), window.outer_size()) else {
        return FAR_POLL;
    };
    let scale = window.scale_factor().unwrap_or(1.0);

    // Cursor-space window rect: physical pixels on Windows (GetCursorPos),
    // logical points elsewhere (enigo reports logical on macOS/Linux — same
    // convention as `get_monitor_with_cursor`).
    let (wx, wy, ww, wh) = cursor_space_rect(origin.x, origin.y, size.width, size.height, scale);
    let (cx, cy) = (cursor.0 as f64, cursor.1 as f64);

    let near = cx >= wx - NEAR_MARGIN
        && cx < wx + ww + NEAR_MARGIN
        && cy >= wy - NEAR_MARGIN
        && cy < wy + wh + NEAR_MARGIN;
    if !near {
        apply(app, &window, true);
        return FAR_POLL;
    }

    let rect = *INTERACTIVE_RECT.lock().unwrap_or_else(|e| e.into_inner());
    let inside = rect.is_some_and(|r| {
        point_in_rect(cx, cy, wx, wy, r, css_to_cursor_factor(scale), RECT_CUSHION)
    });
    apply(app, &window, !inside);
    NEAR_POLL
}

/// Window rect in the coordinate space `cursor_position` reports.
fn cursor_space_rect(x: i32, y: i32, w: u32, h: u32, scale: f64) -> (f64, f64, f64, f64) {
    #[cfg(target_os = "windows")]
    {
        let _ = scale;
        (x as f64, y as f64, w as f64, h as f64)
    }
    #[cfg(not(target_os = "windows"))]
    {
        (
            x as f64 / scale,
            y as f64 / scale,
            w as f64 / scale,
            h as f64 / scale,
        )
    }
}

/// CSS px → cursor-space units. On Windows WebView2 applies the accessibility
/// text scale as a page zoom on top of DPI scaling; elsewhere 1 CSS px is one
/// logical point.
fn css_to_cursor_factor(scale: f64) -> f64 {
    #[cfg(target_os = "windows")]
    {
        scale * super::win32::windows_text_scale_factor()
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = scale;
        1.0
    }
}

/// Is `(px, py)` inside `rect` — reported in CSS px relative to the window
/// origin `(wx, wy)` — after conversion by `factor` and inflation by
/// `cushion` (CSS px)? `pub(crate)` for unit tests.
pub(crate) fn point_in_rect(
    px: f64,
    py: f64,
    wx: f64,
    wy: f64,
    rect: FlowbarRect,
    factor: f64,
    cushion: f64,
) -> bool {
    let rx = wx + (rect.x - cushion) * factor;
    let ry = wy + (rect.y - cushion) * factor;
    let rw = (rect.width + 2.0 * cushion) * factor;
    let rh = (rect.height + 2.0 * cushion) * factor;
    px >= rx && px < rx + rw && py >= ry && py < ry + rh
}

/// Flip `set_ignore_cursor_events` only on real transitions and emit
/// `flowbar://cursor` (payload = cursor currently over an interactive area),
/// so the webview can start its 400 ms leave timer even when the DOM never
/// delivered a `mouseleave` (the window went click-through mid-hover).
fn apply(app: &AppHandle, window: &tauri::webview::WebviewWindow, ignore: bool) {
    if IGNORING.swap(ignore, Ordering::SeqCst) == ignore {
        return;
    }
    let window = window.clone();
    let _ = app.run_on_main_thread(move || {
        let _ = window.set_ignore_cursor_events(ignore);
        let _ = window.emit("flowbar://cursor", !ignore);
    });
}

/// Cursor position in the coordinate space `cursor_space_rect` produces:
/// physical pixels on Windows, logical points on macOS/Linux.
#[cfg(target_os = "windows")]
fn cursor_position(_app: &AppHandle) -> Option<(i32, i32)> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

    let mut point = POINT::default();
    unsafe { GetCursorPos(&mut point) }
        .ok()
        .map(|_| (point.x, point.y))
}

#[cfg(not(target_os = "windows"))]
fn cursor_position(app: &AppHandle) -> Option<(i32, i32)> {
    crate::input::get_cursor_position(app)
}
