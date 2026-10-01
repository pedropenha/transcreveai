//! Non-activatable toast window (F008, T-062).
//!
//! The toast is a separate small `WebviewWindow` (`window_labels::TOAST`)
//! built with the same non-activating flags as the Flow Bar — transparent,
//! always-on-top, `focusable(false)`, extended `WS_EX_NOACTIVATE | TOOLWINDOW
//! | TOPMOST` on Windows — so it never steals focus (FR-008-07, AC-008-01).
//! Unlike the Flow Bar it does **not** run the click-through poll: the window
//! is resized to its actual content (`toast_set_content_height`) and must keep
//! receiving hover so compact → expanded morphing works.
//!
//! Event flow (contracts.md §5):
//! - `detector://meeting` (emitted by the T-061 detector): a new detection
//!   presents the toast; `{ detection_id, ended: true }` hides it.
//! - `toast://show`: generic warnings from other lanes (mic fallback,
//!   check-in) rendered as the compact informational variant.
//! - `toast://state { collapsed, detection?, notice? }`: re-emitted to **all**
//!   windows on every state change — the toast webview renders it and the
//!   Flow Bar turns `collapsed && content` into the amber reopen dot
//!   (FR-008-10/12).
//!
//! v1 behavior decisions (documented per task brief):
//! - A second detection while a toast is visible **replaces** the current one
//!   (no queue); a re-announced `detection_id` refreshes the payload without
//!   replaying the sound or re-presenting a collapsed toast.
//! - Generic `toast://show` notices bypass DND/fullscreen suppression — they
//!   are warnings the user must see; only meeting detections honor
//!   FR-008-12.
//! - `toast_reopen` from the amber dot always shows the toast (the hover is an
//!   explicit opt-in), even if suppression is still active.

use crate::settings::{self, ToastPosition};
use std::sync::Mutex;
use std::sync::MutexGuard;
use tauri::{AppHandle, Emitter, Listener, Manager};

/// `detector://meeting` payload emitted by the T-061 detector.
pub const DETECTOR_MEETING_EVENT: &str = "detector://meeting";
/// Generic toast request from other lanes (contracts.md §5).
pub const TOAST_SHOW_EVENT: &str = "toast://show";
/// Broadcast toast state — the toast renders it, the Flow Bar reads the
/// collapsed flag for its amber indicator.
pub const TOAST_STATE_EVENT: &str = "toast://state";

/// Logical window width (CSS px). The card fills it minus the stage padding;
/// height is content-driven (`toast_set_content_height`).
pub(crate) const TOAST_WIDTH: f64 = 376.0;
/// Window height used before the webview reports its first content height:
/// stage padding (16×2) + compact card (56) — keep in sync with
/// `TOAST_HEIGHT_COMPACT` in `src/toast/toastView.ts`.
pub(crate) const TOAST_DEFAULT_HEIGHT: f64 = 88.0;
/// Bounds for webview-reported heights so a bogus report can't cover the
/// screen.
const TOAST_MIN_CONTENT_HEIGHT: f64 = 40.0;
const TOAST_MAX_CONTENT_HEIGHT: f64 = 480.0;
/// Clearance between the toast's bottom edge and the Flow Bar's top edge, and
/// the corner margin for `bottom_right` placement (logical px).
const TOAST_FLOWBAR_GAP: f64 = 6.0;
const TOAST_CORNER_MARGIN: f64 = 16.0;

// ---------------------------------------------------------------------------
// Payloads
// ---------------------------------------------------------------------------

/// The meeting detection rendered by the toast (`detector://meeting`).
#[derive(Clone, Debug, serde::Serialize)]
pub struct ToastDetection {
    pub detection_id: String,
    pub app_label: String,
    pub exe: String,
    pub action: Option<String>,
    pub started_at: Option<serde_json::Value>,
}

/// A generic `toast://show` warning/notice.
#[derive(Clone, Debug, serde::Serialize)]
pub struct ToastNotice {
    pub kind: String,
    pub message: String,
    pub action: Option<serde_json::Value>,
}

/// The `toast://state` broadcast payload. Always carries all three keys so
/// frontends can keep one stable type.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct ToastStatePayload {
    pub collapsed: bool,
    pub detection: Option<ToastDetection>,
    pub notice: Option<ToastNotice>,
}

/// Lenient parse of `detector://meeting` — T-061 owns the emitter; unknown
/// fields are ignored and missing ones defaulted (contracts.md §5 plus the
/// task-brief `action` field).
#[derive(Debug, Default, serde::Deserialize)]
struct DetectorMeetingPayload {
    detection_id: Option<String>,
    #[serde(default)]
    ended: bool,
    app_label: Option<String>,
    exe: Option<String>,
    action: Option<String>,
    #[allow(dead_code)]
    icon: Option<String>,
    started_at: Option<serde_json::Value>,
}

#[derive(Debug, Default, serde::Deserialize)]
struct ToastShowPayload {
    kind: Option<String>,
    message: Option<String>,
    action: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Current toast contents — `None`/`None` means nothing to show.
pub struct ToastState {
    pub collapsed: bool,
    pub detection: Option<ToastDetection>,
    pub notice: Option<ToastNotice>,
    /// Last content height (CSS px) reported by the webview.
    pub content_height: f64,
}

impl Default for ToastState {
    fn default() -> Self {
        Self {
            collapsed: false,
            detection: None,
            notice: None,
            content_height: TOAST_DEFAULT_HEIGHT,
        }
    }
}

fn lock_state(app: &AppHandle) -> Option<MutexGuard<'_, ToastState>> {
    // `inner()` unwraps the `State` wrapper so the guard borrows the managed
    // `Mutex` (tied to `app`), not the wrapper local.
    app.try_state::<Mutex<ToastState>>()
        .map(|state| state.inner().lock().unwrap_or_else(|e| e.into_inner()))
}

fn emit_state(app: &AppHandle, state: &ToastState) {
    let payload = ToastStatePayload {
        collapsed: state.collapsed,
        detection: state.detection.clone(),
        notice: state.notice.clone(),
    };
    // `emit` (not `emit_to`) reaches every window: the toast itself, the Flow
    // Bar (amber dot) and any future consumer.
    if let Err(e) = app.emit(TOAST_STATE_EVENT, payload) {
        log::warn!("toast://state emit failed: {e}");
    }
}

// ---------------------------------------------------------------------------
// FR-008-12 suppression (DND / fullscreen / presentation mode)
// ---------------------------------------------------------------------------

/// `QUERY_USER_NOTIFICATION_STATE` values that suppress the toast window
/// (FR-008-12): Busy, D3D full-screen, presentation mode. `QUNS_QUIET_TIME`
/// and friends do NOT suppress — the spec lists these three only.
pub(crate) const QUNS_BUSY: i32 = 2;
pub(crate) const QUNS_RUNNING_D3D_FULL_SCREEN: i32 = 3;
pub(crate) const QUNS_PRESENTATION_MODE: i32 = 4;

/// Source for the OS "do not disturb / fullscreen" signal, behind a trait so
/// tests can drive the decision table without touching Win32.
pub trait NotificationStateSource: Send + Sync {
    /// Raw `QUERY_USER_NOTIFICATION_STATE`; `None` on platforms that cannot
    /// report one (never suppresses).
    fn raw_state(&self) -> Option<i32>;
}

/// Maps a raw notification-state query result to "suppress the toast window".
pub(crate) fn suppresses_toasts(raw: Option<i32>) -> bool {
    matches!(
        raw,
        Some(QUNS_BUSY) | Some(QUNS_RUNNING_D3D_FULL_SCREEN) | Some(QUNS_PRESENTATION_MODE)
    )
}

/// Production source: `SHQueryUserNotificationState` on Windows, no signal
/// elsewhere (macOS Focus modes are a later task; v1 detection is Windows).
#[cfg(target_os = "windows")]
struct WindowsNotificationState;

#[cfg(target_os = "windows")]
impl NotificationStateSource for WindowsNotificationState {
    fn raw_state(&self) -> Option<i32> {
        use windows::Win32::UI::Shell::SHQueryUserNotificationState;
        // SAFETY: the windows crate wraps this parameterless shell query; the
        // FFI surface is only the global notification-state read, with no
        // buffers or lifetimes involved.
        unsafe { SHQueryUserNotificationState() }
            .ok()
            .map(|state| state.0)
    }
}

#[cfg(not(target_os = "windows"))]
struct NoNotificationState;

#[cfg(not(target_os = "windows"))]
impl NotificationStateSource for NoNotificationState {
    fn raw_state(&self) -> Option<i32> {
        None
    }
}

fn platform_notification_source() -> impl NotificationStateSource {
    #[cfg(target_os = "windows")]
    {
        WindowsNotificationState
    }
    #[cfg(not(target_os = "windows"))]
    {
        NoNotificationState
    }
}

/// Whether a meeting toast must be withheld right now (detection still runs —
/// the amber dot still lands on the Flow Bar).
pub(crate) fn meeting_toast_suppressed(source: &dyn NotificationStateSource) -> bool {
    suppresses_toasts(source.raw_state())
}

// ---------------------------------------------------------------------------
// Positioning (pure math + platform glue)
// ---------------------------------------------------------------------------

/// Flow Bar's outer rect in physical pixels, or `None` when it is not on
/// screen (hidden window, or never created).
fn flowbar_rect_physical(app: &AppHandle) -> Option<(f64, f64, f64, f64)> {
    let bar = app.get_webview_window(crate::window_labels::FLOWBAR)?;
    if !bar.is_visible().unwrap_or(false) {
        return None;
    }
    let pos = bar.outer_position().ok()?;
    let size = bar.outer_size().ok()?;
    Some((
        pos.x as f64,
        pos.y as f64,
        size.width as f64,
        size.height as f64,
    ))
}

/// Top-left of the toast window inside `area` — all rects are `(x, y, w, h)`
/// in one unit space (physical px on Windows, monitor logical space
/// elsewhere). The toast's *bottom* edge stays put when it resizes: the card
/// is CSS-anchored to the window's bottom, so expand/collapse never shifts
/// its visual anchor (DESIGN.md §9 — "sem trocar de posição").
pub(crate) fn toast_origin(
    area: (f64, f64, f64, f64),
    flowbar: Option<(f64, f64, f64, f64)>,
    toast: (f64, f64),
    position: ToastPosition,
    gap: f64,
    margin: f64,
) -> (f64, f64) {
    let (ax, ay, aw, ah) = area;
    let (tw, th) = toast;
    let corner = || (ax + aw - tw - margin, ay + ah - th - margin);
    match position {
        ToastPosition::BottomRight => corner(),
        ToastPosition::AboveFlowbar => {
            let Some((fx, fy, fw, fh)) = flowbar else {
                return corner();
            };
            // Centered on the bar, clamped inside the work area.
            let lo = ax + margin;
            let hi = (ax + aw - tw - margin).max(lo);
            let x = (fx + fw / 2.0 - tw / 2.0).clamp(lo, hi);
            // Just above the bar; if the bar is docked so high that the toast
            // would leave the work area, drop it below the bar instead.
            let mut y = fy - gap - th;
            if y < ay + margin {
                y = fy + fh + gap;
            }
            (x, y)
        }
    }
}

/// Compute the toast's physical-pixel bounds on the cursor's monitor.
fn toast_bounds_physical(app: &AppHandle, content_height: f64) -> Option<(i32, i32, i32, i32)> {
    let monitor = crate::overlay::get_monitor_with_cursor(app)?;
    let scale = monitor.scale_factor();
    #[cfg(target_os = "windows")]
    let text_scale = crate::overlay::windows_text_scale_factor();
    #[cfg(not(target_os = "windows"))]
    let text_scale = 1.0;

    let settings = settings::get_settings(app);
    let wa = monitor.work_area();
    let area = (
        wa.position.x as f64,
        wa.position.y as f64,
        wa.size.width as f64,
        wa.size.height as f64,
    );
    let content_scale = scale * text_scale;
    let size = (TOAST_WIDTH * content_scale, content_height * content_scale);
    let gap = TOAST_FLOWBAR_GAP * scale;
    let margin = TOAST_CORNER_MARGIN * scale;
    let (x, y) = toast_origin(
        area,
        flowbar_rect_physical(app),
        size,
        settings.meeting_toast_position,
        gap,
        margin,
    );
    Some((
        x.round() as i32,
        y.round() as i32,
        size.0.round().max(1.0) as i32,
        size.1.round().max(1.0) as i32,
    ))
}

/// Move+size the toast to its computed bounds. Must run on the main thread
/// (same rule as `show_overlay_state` — monitor/cursor queries and window
/// geometry are main-thread work on several backends).
fn place_toast_window(app: &AppHandle, window: &tauri::webview::WebviewWindow) {
    let content_height = lock_state(app)
        .map(|s| s.content_height)
        .unwrap_or(TOAST_DEFAULT_HEIGHT);
    let Some((x, y, w, h)) = toast_bounds_physical(app, content_height) else {
        log::debug!("toast: no monitor for placement");
        return;
    };

    #[cfg(target_os = "windows")]
    {
        if let Err(e) = crate::overlay::set_window_bounds_physical(window, x, y, w, h) {
            log::error!("Failed to place toast window: {e}");
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let Some(monitor) = crate::overlay::get_monitor_with_cursor(app) else {
            return;
        };
        let scale = monitor.scale_factor();
        let _ = window.set_size(tauri::Size::Logical(tauri::LogicalSize {
            width: TOAST_WIDTH,
            height: content_height,
        }));
        let _ = window.set_position(tauri::Position::Logical(tauri::LogicalPosition {
            x: x as f64 / scale,
            y: y as f64 / scale,
        }));
    }
}

fn show_toast_window(app: &AppHandle) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(window) = handle.get_webview_window(crate::window_labels::TOAST) {
            place_toast_window(&handle, &window);
            let _ = window.show();
        }
    });
}

fn hide_toast_window(app: &AppHandle) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(window) = handle.get_webview_window(crate::window_labels::TOAST) {
            let _ = window.hide();
        }
    });
}

// ---------------------------------------------------------------------------
// Presentation
// ---------------------------------------------------------------------------

/// Present a meeting detection. When suppressed (FR-008-12) the window stays
/// hidden and the state lands collapsed — the Flow Bar's amber dot is the
/// whole UI until the user hovers it.
fn present_detection(app: &AppHandle, detection: ToastDetection) {
    let suppressed = meeting_toast_suppressed(&platform_notification_source());
    let replay_sound;
    {
        let Some(mut state) = lock_state(app) else {
            return;
        };
        // A re-announced id never replays the cue; a *new* detection while
        // visible replaces the current one and gets a fresh cue (v1: replace,
        // no queue).
        replay_sound =
            state.detection.as_ref().map(|d| &d.detection_id) != Some(&detection.detection_id);
        state.detection = Some(detection);
        state.notice = None;
        state.collapsed = suppressed;
        emit_state(app, &state);
    }
    if suppressed {
        log::info!("Meeting toast suppressed by DND/fullscreen (amber dot only)");
        hide_toast_window(app);
        return;
    }
    show_toast_window(app);
    if replay_sound {
        crate::audio_feedback::play_toast_notification(app);
    }
}

/// Present a generic `toast://show` notice. Warnings are user-facing errors
/// (mic fallback etc.) and intentionally bypass FR-008-12 suppression.
fn present_notice(app: &AppHandle, notice: ToastNotice) {
    {
        let Some(mut state) = lock_state(app) else {
            return;
        };
        state.notice = Some(notice);
        state.detection = None;
        state.collapsed = false;
        emit_state(app, &state);
    }
    show_toast_window(app);
}

fn handle_detector_event(app: &AppHandle, payload: &str) {
    let parsed = match serde_json::from_str::<DetectorMeetingPayload>(payload) {
        Ok(parsed) => parsed,
        Err(e) => {
            log::warn!("toast: unparseable {DETECTOR_MEETING_EVENT} payload: {e}");
            return;
        }
    };

    if parsed.ended {
        // Meeting finished (FR-008-04): drop the toast only when it is showing
        // that detection (or the event carries no id — be lenient).
        let clear = {
            let Some(mut state) = lock_state(app) else {
                return;
            };
            let matches = match (&state.detection, &parsed.detection_id) {
                (Some(d), Some(id)) => &d.detection_id == id,
                (Some(_), None) => true,
                (None, _) => false,
            };
            if matches {
                state.detection = None;
                state.notice = None;
                state.collapsed = false;
                emit_state(app, &state);
            }
            matches
        };
        if clear {
            hide_toast_window(app);
        }
        return;
    }

    let Some(detection_id) = parsed.detection_id.filter(|id| !id.is_empty()) else {
        log::warn!("toast: {DETECTOR_MEETING_EVENT} without detection_id ignored");
        return;
    };
    // A re-announced detection that the user already let collapse stays
    // collapsed — refresh the payload, keep the amber dot (AC-008-07).
    if let Some(state) = lock_state(app) {
        if state.collapsed
            && state
                .detection
                .as_ref()
                .is_some_and(|d| d.detection_id == detection_id)
        {
            drop(state);
            let Some(mut state) = lock_state(app) else {
                return;
            };
            state.detection = Some(ToastDetection {
                detection_id,
                app_label: parsed.app_label.unwrap_or_default(),
                exe: parsed.exe.unwrap_or_default(),
                action: parsed.action,
                started_at: parsed.started_at,
            });
            emit_state(app, &state);
            return;
        }
    }

    present_detection(
        app,
        ToastDetection {
            detection_id,
            app_label: parsed.app_label.unwrap_or_default(),
            exe: parsed.exe.unwrap_or_default(),
            action: parsed.action,
            started_at: parsed.started_at,
        },
    );
}

fn handle_show_event(app: &AppHandle, payload: &str) {
    let parsed = match serde_json::from_str::<ToastShowPayload>(payload) {
        Ok(parsed) => parsed,
        Err(e) => {
            log::warn!("toast: unparseable {TOAST_SHOW_EVENT} payload: {e}");
            return;
        }
    };
    let Some(message) = parsed.message.filter(|m| !m.trim().is_empty()) else {
        log::warn!("toast: {TOAST_SHOW_EVENT} without message ignored");
        return;
    };
    present_notice(
        app,
        ToastNotice {
            kind: parsed.kind.unwrap_or_else(|| "info".to_string()),
            message,
            action: parsed.action,
        },
    );
}

// ---------------------------------------------------------------------------
// Commands (invoked via `commands::toast::*`; wired in lib.rs)
// ---------------------------------------------------------------------------

/// `toast_set_collapsed` — the webview's 60 s idle timer (FR-008-10): hide
/// the window, keep the detection, broadcast `collapsed` so the Flow Bar
/// shows the amber reopen dot.
pub fn set_collapsed(app: &AppHandle, collapsed: bool) {
    if !collapsed {
        reopen(app);
        return;
    }
    let has_content = {
        let Some(mut state) = lock_state(app) else {
            return;
        };
        let has_content = state.detection.is_some() || state.notice.is_some();
        if has_content {
            state.collapsed = true;
            emit_state(app, &state);
        }
        has_content
    };
    if has_content {
        hide_toast_window(app);
    }
}

/// `toast_reopen` — hover on the Flow Bar's amber dot (FR-008-10). Explicit
/// user gesture: does not re-check suppression.
pub fn reopen(app: &AppHandle) {
    let has_content = {
        let Some(mut state) = lock_state(app) else {
            return;
        };
        let has_content = state.detection.is_some() || state.notice.is_some();
        if has_content {
            state.collapsed = false;
            emit_state(app, &state);
        }
        has_content
    };
    if has_content {
        show_toast_window(app);
    }
}

/// `toast_dismiss` — ✕, a finished confirmation, or a post-action cleanup:
/// clear the content, hide the window, clear the amber dot.
pub fn dismiss(app: &AppHandle) {
    {
        let Some(mut state) = lock_state(app) else {
            return;
        };
        state.detection = None;
        state.notice = None;
        state.collapsed = false;
        emit_state(app, &state);
    }
    hide_toast_window(app);
}

/// T-069: clear a pending notice only when it still is the given `kind`
/// (e.g. `meeting_auto_stop`). A lapse/user-stop racing a fresh detection
/// must not wipe the toast that replaced the notice.
pub fn dismiss_notice_if_kind(app: &AppHandle, kind: &str) {
    let cleared = {
        let Some(mut state) = lock_state(app) else {
            return;
        };
        if !state.notice.as_ref().is_some_and(|n| n.kind == kind) {
            return;
        }
        state.notice = None;
        state.collapsed = false;
        emit_state(app, &state);
        true
    };
    if cleared
        && lock_state(app)
            .map(|s| s.detection.is_none() && s.notice.is_none())
            .unwrap_or(true)
    {
        hide_toast_window(app);
    }
}

/// `toast_set_content_height` — the webview reports its rendered height (CSS
/// px) so the native window is only ever as tall as the toast card; the
/// bottom edge stays anchored (the card grows upward).
pub fn set_content_height(app: &AppHandle, height: f64) {
    let height = height.clamp(TOAST_MIN_CONTENT_HEIGHT, TOAST_MAX_CONTENT_HEIGHT);
    {
        let Some(mut state) = lock_state(app) else {
            return;
        };
        if (state.content_height - height).abs() < f64::EPSILON {
            return;
        }
        state.content_height = height;
    }
    // Resize only while visible; the next show places with the stored height.
    let visible = app
        .get_webview_window(crate::window_labels::TOAST)
        .is_some_and(|w| w.is_visible().unwrap_or(false));
    if !visible {
        return;
    }
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(window) = handle.get_webview_window(crate::window_labels::TOAST) {
            place_toast_window(&handle, &window);
        }
    });
}

// ---------------------------------------------------------------------------
// Window + wiring
// ---------------------------------------------------------------------------

/// Creates the toast window, hidden. Built eagerly at startup (not lazily):
/// the toast must appear in ≤ a few hundred ms after a detection and a
/// `WebviewWindowBuilder` cold start (WebView2 bootstrap) is way past that —
/// same reason the Flow Bar is created up front.
fn create_toast_window(app_handle: &AppHandle) {
    let mut builder = tauri::WebviewWindowBuilder::new(
        app_handle,
        crate::window_labels::TOAST,
        tauri::WebviewUrl::App("src/toast/index.html".into()),
    )
    .title("Meeting Toast")
    .resizable(false)
    .inner_size(TOAST_WIDTH, TOAST_DEFAULT_HEIGHT)
    .shadow(false)
    .maximizable(false)
    .minimizable(false)
    .closable(false)
    .accept_first_mouse(true)
    .decorations(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .transparent(true)
    .focusable(false)
    .focused(false)
    .visible(false);

    if let Some(data_dir) = crate::portable::data_dir() {
        builder = builder.data_directory(data_dir.join("webview"));
    }

    match builder.build() {
        Ok(window) => {
            #[cfg(target_os = "windows")]
            crate::overlay::apply_overlay_extended_styles(&window);
            log::debug!("Toast window created (hidden)");
        }
        Err(e) => {
            log::error!("Failed to create toast window: {e}");
        }
    }
}

/// Startup wiring: window + event listeners. Called from `lib.rs::setup`.
pub fn init(app: &AppHandle) {
    app.manage(Mutex::new(ToastState::default()));
    create_toast_window(app);

    let app_handle = app.clone();
    app.listen(DETECTOR_MEETING_EVENT, move |event| {
        handle_detector_event(&app_handle, event.payload());
    });
    let app_handle = app.clone();
    app.listen(TOAST_SHOW_EVENT, move |event| {
        handle_show_event(&app_handle, event.payload());
    });
}

#[cfg(test)]
mod tests;
