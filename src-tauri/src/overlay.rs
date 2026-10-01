use crate::settings;
use crate::settings::{AppSettings, FlowbarVisibility, OverlayStyle};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Listener, Manager};

#[cfg(not(target_os = "macos"))]
use log::debug;

#[cfg(not(target_os = "macos"))]
use tauri::WebviewWindowBuilder;

mod click_through;
mod geometry;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
mod positioning;
#[cfg(target_os = "windows")]
mod win32;

#[cfg(test)]
mod tests;

pub(crate) use click_through::set_interactive_rect;
pub use click_through::FlowbarRect;
// Shared with the toast window (T-062): same monitor pick and Windows
// extended styles / text-scale conventions as the Flow Bar.
pub(crate) use positioning::get_monitor_with_cursor;
#[cfg(target_os = "windows")]
pub(crate) use win32::{
    apply_overlay_extended_styles, set_window_bounds_physical, windows_text_scale_factor,
};

#[cfg(target_os = "macos")]
pub(crate) use macos::create_recording_overlay;

#[cfg(target_os = "linux")]
use geometry::effective_edge;
use positioning::calculate_overlay_position;
#[cfg(not(target_os = "windows"))]
use positioning::current_overlay_logical_size;

// Native overlay window sizes (logical points). One window is reused for every
// state and resized in `show_overlay_state`; each size need only be at least as
// large as the card it hosts. The card is CSS-anchored flush to the docked
// screen edge, so window height doesn't move where the card sits - only the
// edge math in geometry.rs does. Keep these in sync with the card geometry in
// RecordingOverlay.css.
//
// On Windows these sizes are additionally multiplied by the accessibility text
// scale (see win32::windows_text_scale_factor), which WebView2 applies as a
// zoom.
//
// Flow Bar footprint (F001): the idle slit, the hover card with its two
// actions + shortcut tooltip, the recording pill, done/error and
// "nothing-heard" all live inside FLOWBAR_WIDTH x FLOWBAR_HEIGHT - the window
// never resizes during hover (spec: fixed size per visual class, CSS does the
// morphing, no WebView2 flicker).
pub(crate) const FLOWBAR_WIDTH: f64 = 288.0;
pub(crate) const FLOWBAR_HEIGHT: f64 = 104.0;

// Actual is 394x118, just a little extra
const OVERLAY_STREAM_WIDTH: f64 = 400.0;
const OVERLAY_STREAM_HEIGHT: f64 = 120.0;

/// Overlay window size (logical) for a given UI state.
fn overlay_dimensions(state: &str) -> (f64, f64) {
    if state == "streaming" {
        (OVERLAY_STREAM_WIDTH, OVERLAY_STREAM_HEIGHT)
    } else {
        (FLOWBAR_WIDTH, FLOWBAR_HEIGHT)
    }
}

static LAST_MIC_LEVEL_EMIT: AtomicU64 = AtomicU64::new(0);
const EMIT_THROTTLE_MS: u64 = 33; // ~30 FPS

/// Whether the Flow Bar shows its idle slit between sessions (FR-001-10
/// "Sempre" - the v1 default). `overlay_style == None` still removes the
/// overlay entirely: it is the platform/user escape hatch (Linux default).
fn flowbar_always_on(settings: &AppSettings) -> bool {
    settings.overlay_style != OverlayStyle::None
        && settings.flowbar_visibility == FlowbarVisibility::Always
}

/// Timed snooze ("Ocultar por 15/30/60 min", FR-001-07): the field has existed
/// since T-005; T-041 ships the menu that sets it. Runtime hide is
/// `is_flowbar_user_hidden`.
fn flowbar_snoozed(settings: &AppSettings) -> bool {
    settings
        .flowbar_snoozed_until_ms
        .is_some_and(|until| until > crate::tray::now_unix_ms())
}

/// Everything that keeps the Flow Bar off screen right now, persisted or not.
fn flowbar_suppressed(settings: &AppSettings) -> bool {
    settings.overlay_style == OverlayStyle::None
        || settings.flowbar_visibility == FlowbarVisibility::Never
        || flowbar_snoozed(settings)
        || is_flowbar_user_hidden()
}

/// Deadline (unix ms) of the coordinator's current terminal dwell: `done`
/// flashes 600 ms, `error` stays 3 s (DONE_DWELL/ERROR_DWELL in
/// transcription_coordinator/machine.rs - keep in sync). Session-only mode
/// defers the window unmap past the dwell so the outcome is actually visible
/// (AC-001-08); always-on mode never unmaps anyway.
static SESSION_DWELL_UNTIL_MS: AtomicU64 = AtomicU64::new(0);

fn now_unix_ms_u64() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Called from the `session://state` listener registered when the overlay is
/// created: tracks when the Done/Error dwell ends so `hide_recording_overlay`
/// can outwait it. Any non-dwelling state clears the deadline.
pub(crate) fn note_session_state(payload: &str) {
    #[derive(serde::Deserialize)]
    struct SessionStateProbe {
        state: String,
    }
    let Ok(parsed) = serde_json::from_str::<SessionStateProbe>(payload) else {
        return;
    };
    let until = match parsed.state.as_str() {
        "done" => now_unix_ms_u64() + 600,
        "error" => now_unix_ms_u64() + 3_000,
        _ => 0,
    };
    SESSION_DWELL_UNTIL_MS.store(until, Ordering::SeqCst);
}

/// Creates the recording overlay window and keeps it hidden by default
#[cfg(not(target_os = "macos"))]
pub fn create_recording_overlay(app_handle: &AppHandle) {
    // On Linux (Wayland), monitor detection often fails, but we don't need exact coordinates
    // for Layer Shell as we use anchors. On other platforms, we require a monitor.
    #[cfg(not(target_os = "linux"))]
    {
        let position = calculate_overlay_position(app_handle, FLOWBAR_WIDTH, FLOWBAR_HEIGHT);
        if position.is_none() {
            debug!("Failed to determine overlay position, not creating overlay window");
            return;
        }
    }

    // Position starts unset - the show path places the window before mapping.
    let mut builder = WebviewWindowBuilder::new(
        app_handle,
        crate::window_labels::FLOWBAR,
        tauri::WebviewUrl::App("src/overlay/index.html".into()),
    )
    .title("Recording")
    .resizable(false)
    .inner_size(FLOWBAR_WIDTH, FLOWBAR_HEIGHT)
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
            #[cfg(target_os = "linux")]
            {
                // Try to initialize GTK layer shell, ignore errors if compositor doesn't support it
                if linux::init_gtk_layer_shell(&window, FLOWBAR_WIDTH, FLOWBAR_HEIGHT) {
                    debug!("GTK layer shell initialized for overlay window");
                } else {
                    debug!("GTK layer shell not available, falling back to regular window");
                }
            }

            #[cfg(target_os = "windows")]
            win32::apply_overlay_extended_styles(&window);

            // NFR-001-02: the frame is transparent and much larger than the
            // pill - run the click-through hit-test loop (spec notes, F001).
            click_through::start(&window);

            // session://state keeps the dwell tracker warm so session-only
            // mode can outwait the Done/Error dwell before unmapping.
            app_handle.listen(
                crate::transcription_coordinator::SESSION_STATE_EVENT,
                |event| note_session_state(event.payload()),
            );

            debug!("Recording overlay window created successfully (hidden)");
        }
        Err(e) => {
            debug!("Failed to create recording overlay window: {}", e);
        }
    }
}

fn show_overlay_state(app_handle: &AppHandle, state: &str) {
    // Whether the overlay shows at all is governed by overlay_style +
    // flowbar_visibility; position only chooses the docked edge. Checked here
    // (off the main thread) so the common overlay-disabled case never pays for
    // a main-thread hop. A user hide from the tray ("Ocultar Flow Bar",
    // FR-010-14) or a timed snooze suppresses it just as completely - hotkeys
    // keep working, feedback moves to the tray icon (FR-001-10).
    let settings = settings::get_settings(app_handle);
    if flowbar_suppressed(&settings) {
        return;
    }

    // The rest queries monitors and the cursor and mutates window geometry. On
    // Linux the monitor/cursor lookups hit GDK/Xlib on the process's shared X11
    // connection, which is only safe from the GTK main thread - running them on
    // a background thread corrupts the connection and hard-crashes the app
    // (issue #227). Hop to the main thread on every platform to keep the
    // geometry path uniform (a no-op cost on Windows, and it also keeps macOS's
    // NSScreen access main-thread-correct). run_on_main_thread runs the closure
    // inline when already on the main thread, so this never deadlocks.
    let handle = app_handle.clone();
    let state = state.to_string();
    let _ = app_handle.run_on_main_thread(move || show_overlay_state_on_main(&handle, &state));
}

fn show_overlay_state_on_main(app_handle: &AppHandle, state: &str) {
    // Size the overlay for this state (compact vs. streaming), then position it.
    let (width, height) = overlay_dimensions(state);
    if let Some(overlay_window) = app_handle.get_webview_window(crate::window_labels::FLOWBAR) {
        // Invalidate any delayed hide still in flight from a previous session
        // (see `hide_recording_overlay`).
        OVERLAY_SHOW_GENERATION.fetch_add(1, Ordering::SeqCst);

        #[cfg(target_os = "linux")]
        let shown_with_layer_shell = if linux::LAYER_SHELL_ACTIVE.load(Ordering::SeqCst) {
            let edge = effective_edge(&settings::get_settings(app_handle));
            match overlay_window.gtk_window() {
                Ok(gtk_window) => {
                    linux::configure_layer_shell_surface(&gtk_window, edge, width, height)
                }
                Err(error) => log::error!("Failed to access GTK overlay window: {error}"),
            }
            let _ = overlay_window.show();
            true
        } else {
            false
        };
        #[cfg(not(target_os = "linux"))]
        let shown_with_layer_shell = false;

        if !shown_with_layer_shell {
            let size_started = std::time::Instant::now();
            #[cfg(not(target_os = "windows"))]
            let _ =
                overlay_window.set_size(tauri::Size::Logical(tauri::LogicalSize { width, height }));
            #[cfg(target_os = "windows")]
            win32::WINDOWS_OVERLAY_IS_STREAMING.store(state == "streaming", Ordering::Relaxed);
            let size_elapsed = size_started.elapsed();

            let pos_started = std::time::Instant::now();
            #[cfg(not(target_os = "windows"))]
            let set_pos_elapsed =
                if let Some((x, y)) = calculate_overlay_position(app_handle, width, height) {
                    let set_pos_started = std::time::Instant::now();
                    let _ = overlay_window
                        .set_position(tauri::Position::Logical(tauri::LogicalPosition { x, y }));
                    set_pos_started.elapsed()
                } else {
                    std::time::Duration::ZERO
                };
            #[cfg(target_os = "windows")]
            let set_pos_elapsed = {
                let set_pos_started = std::time::Instant::now();
                if let Err(error) =
                    win32::place_windows_overlay(app_handle, &overlay_window, width, height)
                {
                    log::error!("Failed to place recording overlay: {error}");
                }
                set_pos_started.elapsed()
            };
            let pos_calc_elapsed = pos_started.elapsed() - set_pos_elapsed;

            let show_started = std::time::Instant::now();
            let _ = overlay_window.show();
            let show_elapsed = show_started.elapsed();

            // On Windows, aggressively re-assert "topmost" in the native Z-order after showing
            #[cfg(target_os = "windows")]
            win32::force_overlay_topmost(&overlay_window);

            // Re-assert bounds after show(): the pre-show move crosses the DPI
            // boundary, and tao's WM_DPICHANGED reflow clobbers the first placement.
            #[cfg(target_os = "windows")]
            if let Err(error) =
                win32::place_windows_overlay(app_handle, &overlay_window, width, height)
            {
                log::error!("Failed to re-assert recording overlay position: {error}");
            }

            log::debug!(
                "overlay '{}': set_size={:?} pos_calc={:?} set_pos={:?} show={:?}",
                state,
                size_elapsed,
                pos_calc_elapsed,
                set_pos_elapsed,
                show_elapsed
            );
        }

        let _ = overlay_window.emit("show-overlay", state);
    }
}

/// Notify the visible recording overlay that the input stream has delivered its
/// first sample chunk. Audio feedback uses the same backend readiness signal,
/// but this targeted event is skipped when overlays are disabled.
pub fn emit_recording_ready(app_handle: &AppHandle) {
    if !OVERLAY_ENABLED.load(Ordering::Relaxed) || is_flowbar_user_hidden() {
        return;
    }

    // Showing the overlay is also queued onto the main thread. Queue readiness
    // there as well so a very fast always-on stream cannot overtake show-overlay
    // and then get reset back to the arming state by the frontend.
    let handle = app_handle.clone();
    let _ = app_handle.run_on_main_thread(move || {
        let _ = handle.emit_to(crate::window_labels::FLOWBAR, "recording-ready", ());
    });
}

/// Shows the recording overlay window with fade-in animation
pub fn show_recording_overlay(app_handle: &AppHandle) {
    show_overlay_state(app_handle, "recording");
}

/// Shows the larger streaming overlay that displays live transcription text
pub fn show_streaming_overlay(app_handle: &AppHandle) {
    show_overlay_state(app_handle, "streaming");
}

/// Shows the transcribing overlay window
pub fn show_transcribing_overlay(app_handle: &AppHandle) {
    show_overlay_state(app_handle, "transcribing");
}

/// Shows the processing overlay window
pub fn show_processing_overlay(app_handle: &AppHandle) {
    show_overlay_state(app_handle, "processing");
}

/// How long the "Nada ouvido" notice stays on the Flow Bar (FR-002-14).
const NOTHING_HEARD_DISPLAY: Duration = Duration::from_secs(1);

/// FR-002-14: a session under the speech floor - or with no speech detected -
/// is discarded instead of transcribed, and the Flow Bar flashes
/// "Nada ouvido" briefly. The timed hide is guarded by the show generation so
/// a session that reclaimed the overlay meanwhile is never hidden by it.
pub fn show_nothing_heard_overlay(app_handle: &AppHandle) {
    // Same gates as any session state: suppressed means no visual feedback at
    // all (FR-001-10 moves feedback to the tray icon).
    let settings = settings::get_settings(app_handle);
    if flowbar_suppressed(&settings) {
        return;
    }

    let handle = app_handle.clone();
    let _ = app_handle.run_on_main_thread(move || {
        show_overlay_state_on_main(&handle, "nothing-heard");
        // Snapshot AFTER this show bumped the generation: the delayed hide
        // fires only if no newer session re-showed the overlay by then.
        let generation = OVERLAY_SHOW_GENERATION.load(Ordering::SeqCst);
        let handle = handle.clone();
        std::thread::spawn(move || {
            std::thread::sleep(NOTHING_HEARD_DISPLAY);
            if OVERLAY_SHOW_GENERATION.load(Ordering::SeqCst) == generation {
                hide_recording_overlay(&handle);
            }
        });
    });
}

/// F001/FR-001-10: with `flowbar_visibility == Always` (the v1 default) the
/// Flow Bar lives on screen - show the idle slit so the instrument is present
/// before any session starts. Called at startup (the window was just created)
/// and when the tray's "Mostrar Flow Bar" lifts the user hide. A suppressed
/// bar is unmapped quietly - this also covers a settings flip while it was up.
pub fn apply_flowbar_presence(app_handle: &AppHandle) {
    let settings = settings::get_settings(app_handle);
    if flowbar_suppressed(&settings) {
        if let Some(window) = app_handle.get_webview_window(crate::window_labels::FLOWBAR) {
            let _ = window.emit("hide-overlay", ());
            let _ = window.hide();
        }
        return;
    }
    if flowbar_always_on(&settings) {
        show_overlay_state(app_handle, "idle");
    }
}

/// Updates the overlay window position based on current settings
pub fn update_overlay_position(app_handle: &AppHandle) {
    // Positioning queries monitors/cursor (GDK/Xlib on Linux) and moves the
    // window, so it must run on the main thread - see show_overlay_state.
    let handle = app_handle.clone();
    let _ = app_handle.run_on_main_thread(move || update_overlay_position_on_main(&handle));
}

fn update_overlay_position_on_main(app_handle: &AppHandle) {
    if let Some(overlay_window) = app_handle.get_webview_window(crate::window_labels::FLOWBAR) {
        #[cfg(target_os = "linux")]
        if linux::LAYER_SHELL_ACTIVE.load(Ordering::SeqCst) {
            let edge = effective_edge(&settings::get_settings(app_handle));
            match overlay_window.gtk_window() {
                Ok(gtk_window) => linux::configure_layer_shell_position(&gtk_window, edge),
                Err(error) => log::error!("Failed to access GTK overlay window: {error}"),
            }
            return;
        }

        #[cfg(target_os = "windows")]
        {
            let state = if win32::WINDOWS_OVERLAY_IS_STREAMING.load(Ordering::Relaxed) {
                "streaming"
            } else {
                "recording"
            };
            let (width, height) = overlay_dimensions(state);
            if let Err(error) =
                win32::place_windows_overlay(app_handle, &overlay_window, width, height)
            {
                log::error!("Failed to update recording overlay position: {error}");
            }
        }

        #[cfg(not(target_os = "windows"))]
        {
            // Use the window's current size so centering stays correct whether the
            // overlay is in compact or streaming layout.
            let (width, height) = current_overlay_logical_size(&overlay_window)
                .unwrap_or((FLOWBAR_WIDTH, FLOWBAR_HEIGHT));
            if let Some((x, y)) = calculate_overlay_position(app_handle, width, height) {
                let _ = overlay_window
                    .set_position(tauri::Position::Logical(tauri::LogicalPosition { x, y }));
            }
        }
    }
}

/// Generation counter bumped every time the overlay is shown. The deferred
/// `hide()` below only unmaps the window if no show happened after it was
/// scheduled, so a hide left over from a finished transcription can never
/// take down the overlay of a session that started in the meantime - e.g. a
/// press the coordinator remembered while the pipeline was busy and started
/// the instant it drained, well inside the 300 ms hide delay.
static OVERLAY_SHOW_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Fade the card out, then unmap the overlay once the session UI has settled.
///
/// Two refinements over a fixed 300 ms unmap:
/// - always-on Flow Bar (FR-001-10): the window never unmaps at session end -
///   the webview swaps the pill for the idle slit (or keeps showing the
///   Done/Error dwell until `session://state` returns to idle);
/// - session-only: the unmap outwaits the coordinator's terminal dwell
///   (tracked via `SESSION_DWELL_UNTIL_MS`, re-read every tick because the
///   `session://state` event can land just after this call) so the outcome
///   stays visible - that is what makes the Error state perceivable
///   (AC-001-08) when the bar is not always on.
pub fn hide_recording_overlay(app_handle: &AppHandle) {
    // Always hide the overlay regardless of settings - if setting was changed while recording,
    // we still want to hide it properly
    if let Some(overlay_window) = app_handle.get_webview_window(crate::window_labels::FLOWBAR) {
        // Snapshot before doing anything observable, so any show that lands
        // after this point invalidates the deferred hide below.
        let scheduled_at = OVERLAY_SHOW_GENERATION.load(Ordering::SeqCst);
        // Emit event to trigger fade-out animation
        let _ = overlay_window.emit("hide-overlay", ());

        if flowbar_always_on(&settings::get_settings(app_handle)) {
            return;
        }

        let window_clone = overlay_window.clone();
        std::thread::spawn(move || {
            let started = std::time::Instant::now();
            loop {
                if OVERLAY_SHOW_GENERATION.load(Ordering::SeqCst) != scheduled_at {
                    log::debug!(
                        "Skipping stale overlay hide: a newer session is showing the overlay"
                    );
                    return;
                }
                let dwell_left_ms = SESSION_DWELL_UNTIL_MS
                    .load(Ordering::SeqCst)
                    .saturating_sub(now_unix_ms_u64());
                let wait =
                    Duration::from_millis(300).max(Duration::from_millis(dwell_left_ms + 60));
                if started.elapsed() >= wait || started.elapsed() >= Duration::from_secs(6) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            let _ = window_clone.hide();
        });
    }
}

// Cached "overlay is enabled" flag, kept in sync with overlay_style. Avoids
// reading the Tauri store on every audio callback (~24 Hz during recording).
// Defaults to false so the audio path doesn't emit until lib.rs::setup
// populates the cache from initial settings.
static OVERLAY_ENABLED: AtomicBool = AtomicBool::new(false);

/// Update the cached overlay-enabled flag. Called from `lib.rs` at
/// startup after settings load, and from `change_overlay_style_setting`
/// whenever the user changes whether the overlay is shown.
pub fn update_overlay_enabled_cache(enabled: bool) {
    OVERLAY_ENABLED.store(enabled, Ordering::Relaxed);
}

/// User-driven Flow Bar suppression from the tray's "Ocultar Flow Bar"
/// (FR-010-14), matching FR-001-07 "Ocultar ate reiniciar o app": runtime-only
/// and never persisted - a restart always brings the bar back. The persisted
/// `flowbar_snoozed_until_ms` setting covers the timed variant (T-041).
static FLOWBAR_USER_HIDDEN: AtomicBool = AtomicBool::new(false);

/// Whether the tray's "Ocultar Flow Bar" currently suppresses the overlay.
pub fn is_flowbar_user_hidden() -> bool {
    FLOWBAR_USER_HIDDEN.load(Ordering::Relaxed)
}

/// Hide or un-suppress the Flow Bar until the next launch. Hiding unmaps the
/// window immediately - even in always-on mode, this is a user "Ocultar", not
/// a session end. Showing only lifts the suppression: in always-on mode the
/// idle slit reappears at once via `apply_flowbar_presence`.
pub fn set_flowbar_user_hidden(app_handle: &AppHandle, hidden: bool) {
    FLOWBAR_USER_HIDDEN.store(hidden, Ordering::Relaxed);
    if hidden {
        if let Some(window) = app_handle.get_webview_window(crate::window_labels::FLOWBAR) {
            // Same fade -> delayed unmap dance as a normal hide, just forced.
            let scheduled_at = OVERLAY_SHOW_GENERATION.load(Ordering::SeqCst);
            let _ = window.emit("hide-overlay", ());
            let window_clone = window.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(300));
                if OVERLAY_SHOW_GENERATION.load(Ordering::SeqCst) == scheduled_at {
                    let _ = window_clone.hide();
                }
            });
        }
    } else {
        apply_flowbar_presence(app_handle);
    }
}

/// IPC contract (contracts.md §5): the flowbar receives `audio://level` with
/// `{ rms: f32[] }` at ~30 Hz, only while recording. The values are the
/// visualizer's per-band levels - the flowbar renders them directly as bars.
const LEVEL_EVENT: &str = "audio://level";

pub fn emit_levels(app_handle: &AppHandle, levels: &[f32]) {
    // Skip emission when the overlay is disabled. The flowbar
    // window is created at boot regardless of overlay_style, so without this
    // guard a hidden overlay's WebKit subprocess still
    // processes every event. Each event drives some kind of WebKit
    // C++ allocation that accumulates without bound (mechanism not
    // directly characterized; see issue #1279 for the investigation).
    // For users with `overlay_style: none` (the Linux default) this skip
    // eliminates the upstream driver of that accumulation.
    if !OVERLAY_ENABLED.load(Ordering::Relaxed) || is_flowbar_user_hidden() {
        return;
    }

    // Throttle to ~30 Hz (FR-001-05). Even with the overlay enabled, the raw
    // audio callback can fire faster than the UI needs (the visualizer emits
    // one event per ~input_rate/30 samples); capping emission rate
    // cuts the per-frame `eval_script`/IPC volume that drives the wry
    // memory growth in issue #1279 (upstream tauri-apps/wry#1489).
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let last = LAST_MIC_LEVEL_EMIT.load(Ordering::Relaxed);
    if now.saturating_sub(last) < EMIT_THROTTLE_MS {
        return;
    }
    LAST_MIC_LEVEL_EMIT.store(now, Ordering::Relaxed);

    // Target only the overlay window. In Tauri 2 both `AppHandle::emit`
    // and `WebviewWindow::emit` broadcast to all webviews; Tauri's
    // listener filter then skips webviews with no registered listener
    // for the event, so the settings webview never received the level event.
    // But the previous dual-call pattern still produced two `eval_script`
    // calls to the overlay per audio callback (one from each .emit()).
    // `emit_to` with the overlay's window label produces a single
    // eval_script call per callback, cutting the per-callback WebKit
    // dispatch work in half.
    let _ = app_handle.emit_to(
        crate::window_labels::FLOWBAR,
        LEVEL_EVENT,
        serde_json::json!({ "rms": levels }),
    );
}
