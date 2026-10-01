//! Window placement, drag & pin persistence (T-092, FR-012-16 / AC-012-04),
//! the platform-specific window plumbing (FR-012-11) and the panel lifecycle
//! entry points (`init`/`open`/`close`/`focus`/`hotkey`).

use std::sync::Mutex;
use tauri::{AppHandle, Manager};

use super::state::{emit_state, emit_state_locked};
use super::{lock_session, AssistantSession};
use crate::settings::{self, AssistantPanelPosition};
use crate::window_labels::ASSISTANT;

// ---------------------------------------------------------------------------
// Panel geometry (logical px; physical conversion happens per platform)
// ---------------------------------------------------------------------------

/// Floating-panel footprint — generous enough for a chat exchange, compact
/// enough to read as an overlay. Without a persisted position every open
/// recenters on the cursor's monitor (T-092 restores the dragged one).
pub(crate) const PANEL_WIDTH: f64 = 420.0;
pub(crate) const PANEL_HEIGHT: f64 = 560.0;
/// Bottom margin above the taskbar edge of the work area.
pub(crate) const PANEL_BOTTOM_MARGIN: f64 = 72.0;
// ---------------------------------------------------------------------------
// Window (FR-012-11)
// ---------------------------------------------------------------------------

/// Panel origin in the monitor's **physical** pixels: centered horizontally,
/// docked `PANEL_BOTTOM_MARGIN` (logical) above the work-area bottom. Pure
/// math — `work_area` is physical px, `scale` the monitor's DPI factor
/// (times the Windows accessibility text scale on that platform).
pub(crate) fn panel_origin(work_area: (i32, i32, u32, u32), scale: f64) -> (f64, f64) {
    let (ax, ay, aw, ah) = work_area;
    let x = ax as f64 + (aw as f64 - PANEL_WIDTH * scale) / 2.0;
    let y = ay as f64 + ah as f64 - (PANEL_HEIGHT + PANEL_BOTTOM_MARGIN) * scale;
    // Clamp to the work area's own origin — a left/above-primary monitor has
    // negative coordinates, and clamping to global 0 would teleport the
    // panel onto the primary screen.
    (x.max(ax as f64), y.max(ay as f64))
}

/// Logical-point origin for the platforms where Tauri positions windows in
/// logical coordinates (macOS, Linux). A persisted drag position (T-092)
/// wins over the default dock.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn panel_logical_origin(app: &AppHandle) -> Option<(f64, f64)> {
    if let Some((x, y, content_scale)) = restored_panel_position(app) {
        // Off Windows `content_scale` is just the monitor's scale factor.
        return Some((x as f64 / content_scale, y as f64 / content_scale));
    }
    let monitor = crate::overlay::get_monitor_with_cursor(app)?;
    let wa = monitor.work_area();
    let scale = monitor.scale_factor();
    let (x, y) = panel_origin(
        (wa.position.x, wa.position.y, wa.size.width, wa.size.height),
        scale,
    );
    Some((x / scale, y / scale))
}

// ---------------------------------------------------------------------------
// Drag & pin (T-092, FR-012-16 / AC-012-04)
// ---------------------------------------------------------------------------

/// The text-size zoom WebView2 applies on top of DPI scaling on Windows —
/// the same convention the window-bounds path uses
/// (`overlay::windows_text_scale_factor`). Webview screen coordinates
/// (`PointerEvent.screenX/Y`) arrive in DIP (physical / DPI scale); client
/// coordinates (`clientX/Y`) arrive in CSS px, so a grab offset must be
/// multiplied by this zoom before it can leave CSS space. `1.0` elsewhere.
#[cfg(target_os = "windows")]
fn panel_text_zoom() -> f64 {
    crate::overlay::windows_text_scale_factor()
}

#[cfg(not(target_os = "windows"))]
fn panel_text_zoom() -> f64 {
    1.0
}

/// Work-area view of one monitor — the pure-data stand-in for
/// `tauri::Monitor` that keeps the placement math unit-testable.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PanelMonitor {
    /// OS monitor name when reported — secondary identity that finds a
    /// monitor whose coordinates moved (resolution/DPI change).
    pub name: Option<String>,
    /// Work area in physical px: `(x, y, width, height)`.
    pub work_area: (i32, i32, i32, i32),
    /// CSS-px → physical-px factor on this monitor (DPI scale × text zoom);
    /// the persisted footprint converts through it.
    pub content_scale: f64,
}

fn panel_monitor(monitor: &tauri::Monitor, text_zoom: f64) -> PanelMonitor {
    let wa = monitor.work_area();
    PanelMonitor {
        name: monitor.name().cloned(),
        work_area: (
            wa.position.x,
            wa.position.y,
            wa.size.width as i32,
            wa.size.height as i32,
        ),
        content_scale: monitor.scale_factor() * text_zoom,
    }
}

/// Two `tauri::Monitor`s describing the same display — position+size+name,
/// the full identity the OS reports.
fn same_monitor(a: &tauri::Monitor, b: &tauri::Monitor) -> bool {
    a.position() == b.position() && a.size() == b.size() && a.name() == b.name()
}

/// Clamp a rect's top-left so the whole `w`×`h` rect fits inside `area`
/// (`(x, y, width, height)`); a rect larger than the area pins to its
/// origin.
pub(crate) fn clamp_origin_to_area(
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    area: (i32, i32, i32, i32),
) -> (i32, i32) {
    let (ax, ay, aw, ah) = area;
    let max_x = ax + (aw - w).max(0);
    let max_y = ay + (ah - h).max(0);
    (x.clamp(ax, max_x), y.clamp(ay, max_y))
}

/// Half-open point-in-rect test in physical px — same convention as
/// `overlay::positioning::is_mouse_within_monitor`.
fn area_contains(area: (i32, i32, i32, i32), px: i32, py: i32) -> bool {
    let (ax, ay, aw, ah) = area;
    px >= ax && px < ax + aw && py >= ay && py < ay + ah
}

/// Where a persisted panel position lands on the live monitor layout —
/// returns `(x, y, monitor_index)` in physical px, or `None` when nothing
/// was saved / no monitor exists (callers then use the default dock).
/// `panel` is the CSS-px footprint (`PANEL_WIDTH`/`PANEL_HEIGHT`); each
/// monitor's `content_scale` converts it.
///
/// Resolution order (FR-012-16 "respeita bordas e multi-monitor"):
/// 1. the monitor whose work area still contains the saved rect's center —
///    the panel stays where the user left it, clamped fully inside;
/// 2. the monitor with the saved name — survives resolution/DPI changes;
///    the saved point is simply clamped back inside;
/// 3. the primary monitor at the same *relative* spot in its work area —
///    the "monitor sumiu" fallback (AC-012-04).
pub(crate) fn resolve_panel_position(
    saved: Option<&AssistantPanelPosition>,
    monitors: &[PanelMonitor],
    primary_index: usize,
    panel: (f64, f64),
) -> Option<(i32, i32, usize)> {
    let saved = saved?;
    if monitors.is_empty() {
        return None;
    }
    let primary_index = primary_index.min(monitors.len() - 1);
    let size_on = |m: &PanelMonitor| {
        (
            (panel.0 * m.content_scale).round().max(1.0) as i32,
            (panel.1 * m.content_scale).round().max(1.0) as i32,
        )
    };
    let target = monitors
        .iter()
        .enumerate()
        .find(|(_, m)| {
            let (w, h) = size_on(m);
            area_contains(m.work_area, saved.x + w / 2, saved.y + h / 2)
        })
        .or_else(|| {
            saved.monitor_name.as_deref().and_then(|name| {
                monitors
                    .iter()
                    .enumerate()
                    .find(|(_, m)| m.name.as_deref() == Some(name))
            })
        });
    let (index, monitor) = target.unwrap_or((primary_index, &monitors[primary_index]));
    let (w, h) = size_on(monitor);
    let (x, y) = if target.is_some() {
        (saved.x, saved.y)
    } else {
        // Monitor gone → the same relative spot in the primary work area.
        let (ax, ay, aw, ah) = monitor.work_area;
        (
            ax + (saved.rel_x.clamp(0.0, 1.0) * aw as f64).round() as i32,
            ay + (saved.rel_y.clamp(0.0, 1.0) * ah as f64).round() as i32,
        )
    };
    let (x, y) = clamp_origin_to_area(x, y, w, h, monitor.work_area);
    Some((x, y, index))
}

/// The persisted position resolved against the current monitors:
/// `(x, y, content_scale)` in physical px — `None` falls back to the
/// default dock on the cursor's monitor.
fn restored_panel_position(app: &AppHandle) -> Option<(i32, i32, f64)> {
    let saved = settings::get_settings(app).assistant_panel_position?;
    let monitors = app.available_monitors().ok()?;
    if monitors.is_empty() {
        return None;
    }
    let text_zoom = panel_text_zoom();
    let areas: Vec<PanelMonitor> = monitors
        .iter()
        .map(|m| panel_monitor(m, text_zoom))
        .collect();
    let primary_index = app
        .primary_monitor()
        .ok()
        .flatten()
        .and_then(|primary| monitors.iter().position(|m| same_monitor(m, &primary)))
        .unwrap_or(0);
    let (x, y, index) = resolve_panel_position(
        Some(&saved),
        &areas,
        primary_index,
        (PANEL_WIDTH, PANEL_HEIGHT),
    )?;
    Some((x, y, areas[index].content_scale))
}

/// A drag-resolved placement: physical bounds plus the target monitor (for
/// the relative-position bookkeeping persisted on drag end).
struct PanelPlacement {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    monitor: PanelMonitor,
}

/// Turn a drag into physical bounds on the monitor under the cursor.
/// `grab_*` is the `clientX/Y` captured where the drag started (CSS px —
/// the pointer's offset inside the window stays constant while the window
/// tracks the cursor). The result is clamped into the monitor's work area.
///
/// The cursor position comes from `AppHandle::cursor_position` — physical
/// px relative to the desktop origin, the same space monitor
/// `position()`/`size()` report. The webview's `screenX/Y` cannot be used:
/// it is DIP, and which monitor's scale divides it is ambiguous mid-drag
/// on mixed-DPI layouts.
fn drag_placement(app: &AppHandle, grab_x: f64, grab_y: f64) -> Option<PanelPlacement> {
    let cursor = app.cursor_position().ok()?;
    let cursor = (cursor.x.round() as i32, cursor.y.round() as i32);
    let monitors = app.available_monitors().ok()?;
    let zoom = panel_text_zoom();
    let target = monitors
        .iter()
        .find(|m| {
            area_contains(
                (
                    m.position().x,
                    m.position().y,
                    m.size().width as i32,
                    m.size().height as i32,
                ),
                cursor.0,
                cursor.1,
            )
        })
        .cloned()
        .or_else(|| app.primary_monitor().ok().flatten())?;
    let monitor = panel_monitor(&target, zoom);
    // The grab offset is measured inside the dragged window in CSS px —
    // convert it through that window's own scale (its monitor's DPI ×
    // text zoom), which can differ from the target monitor's mid-drag.
    let grab_scale = app
        .get_webview_window(ASSISTANT)
        .and_then(|w| w.scale_factor().ok())
        .unwrap_or_else(|| target.scale_factor())
        * zoom;
    let x = (cursor.0 as f64 - grab_x * grab_scale).round() as i32;
    let y = (cursor.1 as f64 - grab_y * grab_scale).round() as i32;
    let w = (PANEL_WIDTH * monitor.content_scale).round().max(1.0) as i32;
    let h = (PANEL_HEIGHT * monitor.content_scale).round().max(1.0) as i32;
    let (x, y) = clamp_origin_to_area(x, y, w, h, monitor.work_area);
    Some(PanelPlacement {
        x,
        y,
        w,
        h,
        monitor,
    })
}

fn apply_panel_placement(app: &AppHandle, placement: &PanelPlacement) {
    #[cfg(target_os = "windows")]
    {
        if let Some(window) = app.get_webview_window(ASSISTANT) {
            // Physical-px SetWindowPos — no activation, no Z-order change
            // (same convention as the show path).
            let _ = crate::overlay::set_window_bounds_physical(
                &window,
                placement.x,
                placement.y,
                placement.w,
                placement.h,
            );
        }
    }
    #[cfg(target_os = "macos")]
    {
        macos::move_panel(
            app,
            placement.x as f64 / placement.monitor.content_scale,
            placement.y as f64 / placement.monitor.content_scale,
        );
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(window) = app.get_webview_window(ASSISTANT) {
            let _ = window.set_position(tauri::Position::Logical(tauri::LogicalPosition {
                x: placement.x as f64 / placement.monitor.content_scale,
                y: placement.y as f64 / placement.monitor.content_scale,
            }));
        }
    }
}

/// `assistant_move_panel` / `assistant_save_panel_position` shared body
/// (FR-012-16): clamp the drag point onto the monitor under the cursor and
/// move the window. `persist` — only true on drag end, so the settings
/// store is written once per drag, not at pointer-move rate. A pinned
/// panel ignores every drag (visible but immovable).
pub fn move_panel(app: &AppHandle, grab_x: f64, grab_y: f64, persist: bool) {
    let mut settings = settings::get_settings(app);
    if settings.assistant_panel_pinned {
        return;
    }
    let Some(placement) = drag_placement(app, grab_x, grab_y) else {
        return;
    };
    if persist {
        let (ax, ay, aw, ah) = placement.monitor.work_area;
        settings.assistant_panel_position = Some(AssistantPanelPosition {
            x: placement.x,
            y: placement.y,
            rel_x: ((placement.x - ax) as f64 / aw.max(1) as f64).clamp(0.0, 1.0),
            rel_y: ((placement.y - ay) as f64 / ah.max(1) as f64).clamp(0.0, 1.0),
            monitor_name: placement.monitor.name.clone(),
        });
        settings::write_settings(app, settings);
    }
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || apply_panel_placement(&handle, &placement));
}

#[cfg(target_os = "macos")]
mod macos {
    use super::{PANEL_HEIGHT, PANEL_WIDTH};
    use tauri::{AppHandle, WebviewUrl};
    use tauri_nspanel::{
        tauri_panel, CollectionBehavior, ManagerExt, PanelBuilder, PanelLevel, StyleMask,
    };

    tauri_panel! {
        panel!(AssistantPanel {
            config: {
                // Unlike the Flow Bar this panel must accept keyboard input —
                // it only becomes key on explicit intent (click/dictation),
                // never on `show` (nonactivating_panel + no_activate).
                can_become_key_window: true,
                is_floating_panel: true
            }
        })
    }

    /// Creates the assistant panel, hidden — same cold-start reasoning as the
    /// Flow Bar (a WebView can't boot inside the open budget).
    pub(super) fn create_assistant_panel(app_handle: &AppHandle) {
        match PanelBuilder::<_, AssistantPanel>::new(app_handle, crate::window_labels::ASSISTANT)
            .url(WebviewUrl::App("src/assistant/index.html".into()))
            .title("Assistente")
            .level(PanelLevel::Status)
            .size(tauri::Size::Logical(tauri::LogicalSize {
                width: PANEL_WIDTH,
                height: PANEL_HEIGHT,
            }))
            .has_shadow(true)
            .transparent(true)
            .no_activate(true)
            .corner_radius(0.0)
            .style_mask(StyleMask::empty().borderless().nonactivating_panel())
            .with_window(|w| w.decorations(false).transparent(true))
            .collection_behavior(
                CollectionBehavior::new()
                    .can_join_all_spaces()
                    .full_screen_auxiliary(),
            )
            .build()
        {
            Ok(panel) => {
                panel.hide();
                log::debug!("Assistant panel created (hidden)");
            }
            Err(e) => {
                log::error!("Failed to create assistant panel: {e}");
            }
        }
    }

    /// orderFrontRegardless — appears topmost with zero focus change.
    pub(super) fn show(app: &AppHandle) {
        if let Ok(panel) = app.get_webview_panel(crate::window_labels::ASSISTANT) {
            if let (Some(origin), Some(window)) =
                (super::panel_logical_origin(app), panel.to_window())
            {
                let _ = window.set_position(tauri::Position::Logical(tauri::LogicalPosition {
                    x: origin.0,
                    y: origin.1,
                }));
            }
            panel.show();
        }
    }

    /// T-092 drag: reposition the panel (logical points — off Windows the
    /// placement math already resolved monitor + clamp).
    pub(super) fn move_panel(app: &AppHandle, x: f64, y: f64) {
        if let Ok(panel) = app.get_webview_panel(crate::window_labels::ASSISTANT) {
            if let Some(window) = panel.to_window() {
                let _ =
                    window.set_position(tauri::Position::Logical(tauri::LogicalPosition { x, y }));
            }
        }
    }

    pub(super) fn hide(app: &AppHandle) {
        if let Ok(panel) = app.get_webview_panel(crate::window_labels::ASSISTANT) {
            panel.hide();
        }
    }

    /// The nonactivating style mask means a click alone never makes the panel
    /// key — the webview calls `assistant_focus` on mousedown and we take key
    /// status here (and on assistant-routed dictation).
    pub(super) fn focus(app: &AppHandle) {
        if let Ok(panel) = app.get_webview_panel(crate::window_labels::ASSISTANT) {
            panel.make_key_window();
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn create_assistant_window(app: &AppHandle) {
    let mut builder = tauri::WebviewWindowBuilder::new(
        app,
        ASSISTANT,
        tauri::WebviewUrl::App("src/assistant/index.html".into()),
    )
    .title("Assistente")
    .resizable(false)
    .inner_size(PANEL_WIDTH, PANEL_HEIGHT)
    .shadow(true)
    .maximizable(false)
    .minimizable(false)
    .accept_first_mouse(true)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .transparent(true)
    // Focusable — unlike the Flow Bar this panel accepts keyboard input once
    // the user clicks it (no WS_EX_NOACTIVATE on this window).
    .focusable(true)
    .focused(false)
    .visible(false);

    if let Some(data_dir) = crate::portable::data_dir() {
        builder = builder.data_directory(data_dir.join("webview"));
    }

    match builder.build() {
        Ok(_window) => {
            log::debug!("Assistant window created (hidden)");
        }
        Err(e) => {
            log::error!("Failed to create assistant window: {e}");
        }
    }
}

/// Show + topmost with **no activation** (FR-012-11). Windows goes through
/// `SetWindowPos` so the window surfaces without a foreground switch;
/// Linux's `window.show()` is best-effort (wry activates on some WMs).
#[cfg(target_os = "windows")]
fn show_native(app: &AppHandle) {
    let Some(window) = app.get_webview_window(ASSISTANT) else {
        return;
    };
    if let Some((x, y, content_scale)) = restored_panel_position(app) {
        // AC-012-04: a dragged+persisted position wins — already resolved
        // against the live monitors (clamp / named-monitor / primary
        // fallback all happened inside).
        let width = (PANEL_WIDTH * content_scale).round().max(1.0) as i32;
        let height = (PANEL_HEIGHT * content_scale).round().max(1.0) as i32;
        if let Err(e) = crate::overlay::set_window_bounds_physical(&window, x, y, width, height) {
            log::warn!("assistant: failed to restore panel position: {e}");
        }
    } else if let Some(monitor) = crate::overlay::get_monitor_with_cursor(app) {
        let wa = monitor.work_area();
        // Same WebView2 text-zoom convention as the Flow Bar (win32.rs).
        let scale = monitor.scale_factor() * crate::overlay::windows_text_scale_factor();
        let width = (PANEL_WIDTH * scale).round().max(1.0) as i32;
        let height = (PANEL_HEIGHT * scale).round().max(1.0) as i32;
        let (x, y) = panel_origin(
            (wa.position.x, wa.position.y, wa.size.width, wa.size.height),
            scale,
        );
        if let Err(e) =
            crate::overlay::set_window_bounds_physical(&window, x as i32, y as i32, width, height)
        {
            log::warn!("assistant: failed to place panel: {e}");
        }
    }
    // SWP_SHOWWINDOW | SWP_NOACTIVATE | HWND_TOPMOST — surfaces the hidden
    // window without touching the foreground.
    crate::overlay::force_overlay_topmost(&window);
}

#[cfg(target_os = "linux")]
fn show_native(app: &AppHandle) {
    let Some(window) = app.get_webview_window(ASSISTANT) else {
        return;
    };
    if let Some((x, y)) = panel_logical_origin(app) {
        let _ = window.set_position(tauri::Position::Logical(tauri::LogicalPosition { x, y }));
    }
    let _ = window.set_always_on_top(true);
    let _ = window.show();
}

#[cfg(target_os = "macos")]
fn show_native(app: &AppHandle) {
    macos::show(app);
}

#[cfg(not(target_os = "macos"))]
fn hide_native(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(ASSISTANT) {
        let _ = window.hide();
    }
}

#[cfg(target_os = "macos")]
fn hide_native(app: &AppHandle) {
    macos::hide(app);
}

/// Take keyboard focus on explicit user intent only (FR-012-11).
fn focus_native(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    {
        macos::focus(app);
    }
    #[cfg(not(target_os = "macos"))]
    {
        if let Some(window) = app.get_webview_window(ASSISTANT) {
            let _ = window.set_focus();
        }
    }
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

/// Startup wiring — session state + the hidden window. Called from
/// `lib.rs::initialize_core_logic` after `toast::init`.
pub fn init(app: &AppHandle) {
    app.manage(Mutex::new(AssistantSession::default()));
    #[cfg(target_os = "macos")]
    macos::create_assistant_panel(app);
    #[cfg(not(target_os = "macos"))]
    create_assistant_window(app);
}

/// Open the panel (no-op when already open). Used by the hotkey's first
/// press and the Flow Bar button.
pub fn open_panel(app: &AppHandle) {
    let was_open = {
        let Some(mut session) = lock_session(app) else {
            return;
        };
        if session.open {
            true
        } else {
            session.open = true;
            false
        }
    };
    if was_open {
        return;
    }
    // Show first, then emit: the state event must land on a webview that is
    // already surfacing — emitting before the show is scheduled leaves a
    // hidden webview consuming an `open: true` snapshot out of order.
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || show_native(&handle));
    emit_state(app);
}

/// Closing the panel mid-dictation is the abort gesture — stop the routed
/// capture too, or the mic would keep recording invisibly until the next
/// hotkey press. `cancel_current_operation` clears the claim via
/// `note_dictation_cancelled`; the final text is then dropped at `stop`,
/// never pasted elsewhere (FR-012-12 `DictationRoute::Drop`).
fn abort_routed_dictation(app: &AppHandle) {
    let routed = lock_session(app)
        .map(|s| s.dictation_routed || s.dictating)
        .unwrap_or(false);
    if routed {
        crate::utils::cancel_current_operation(app);
    }
}

/// Hide the panel; the session (history, draft-independent state) survives —
/// the conversation is cleared only by `new_conversation` (FR-012-15).
pub fn close_panel(app: &AppHandle) {
    {
        let Some(mut session) = lock_session(app) else {
            return;
        };
        session.open = false;
        emit_state_locked(app, &mut session);
    }
    abort_routed_dictation(app);
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || hide_native(&handle));
}

/// The OS-level close path (`on_window_event` hides the window itself) —
/// keep the session flag honest so the next hotkey press re-opens instead of
/// sending.
pub fn note_panel_hidden(app: &AppHandle) {
    {
        let Some(mut session) = lock_session(app) else {
            return;
        };
        if session.open {
            session.open = false;
            emit_state_locked(app, &mut session);
        }
    }
    abort_routed_dictation(app);
}

/// Explicit user intent to interact — click (`assistant_focus`) or a routed
/// dictation start (`maybe_claim_dictation`).
pub fn focus_panel(app: &AppHandle) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || focus_native(&handle));
}
