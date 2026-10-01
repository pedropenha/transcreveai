//! System tray icon and menu.
//!
//! The tray is driven by a single *desired state* snapshot ([`TrayDesired`])
//! that callers update through [`set_tray_state`], [`refresh_tray_icon`] and
//! [`update_tray_menu`]. Every such call just records intent and schedules a
//! single applier on the main thread, which diffs the desired snapshot against
//! what is currently displayed and touches the native tray only for the parts
//! that actually changed. Requests that arrive while an apply is pending are
//! coalesced into it, so bursts of state changes never queue up native work.
//!
//! Why: native tray updates are the lever we control for the macOS tray
//! disappearance bug (tauri-apps/tauri#12060, Handy #1948). Before this, every
//! recording cycle rebuilt the full menu 3-6 times from several threads, and
//! concurrent rebuilds could interleave and leave a stale menu behind.
//!
//! Exception: [`set_tray_visibility`] and [`recreate_tray_icon`] call the tray
//! directly. Visibility is a separate attribute that never participates in the
//! icon/menu diff, both are rare and user-initiated, and Tauri marshals them
//! onto the main thread so they serialize with the applier anyway. Re-showing
//! a hidden tray relies on tray-icon recreating it from the last applied
//! icon/menu/tooltip, so those must only ever be set through the applier.

use crate::managers::audio::AudioRecordingManager;
use crate::settings;
use crate::tray_i18n::get_tray_translations;
use log::{debug, error, info, trace, warn};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tauri::image::Image;
use tauri::menu::{CheckMenuItem, IsMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIcon;
use tauri::{AppHandle, Manager, Theme};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayIconState {
    Idle,
    Recording,
    Transcribing,
}

impl TrayIconState {
    /// Recording and Transcribing share the same menu ("Cancel" instead of the
    /// model submenu), so only the idle/busy distinction matters for the menu.
    fn is_busy(self) -> bool {
        self != TrayIconState::Idle
    }
}

/// Everything the tray *menu* (and tooltip) depends on. When two snapshots
/// compare equal the menu is not rebuilt.
#[derive(Clone, Debug, PartialEq, Eq)]
struct MenuInputs {
    /// Recording or transcribing — adds the "Cancel" row (FR-002).
    busy: bool,
    /// A dictation is recording right now — its entry reads "Stop Dictation"
    /// (FR-010-14). During `Transcribing` the entry flips back to "Start
    /// Dictation": clicking then queues a new session for when the pipeline
    /// drains (the coordinator's remembered-press path).
    recording: bool,
    warning: bool,
    /// A meeting is being recorded (T-064) — switches its entry to "Stop
    /// Meeting" and forces the red recording icon (FR-009-07).
    meeting_active: bool,
    /// Tray "Ocultar Flow Bar" suppression is on (runtime-only, FR-001-07).
    flowbar_hidden: bool,
    /// Meeting detection pause deadline is still in the future (FR-010-14).
    meeting_detection_paused: bool,
    /// `privacy.offline_mode` (FR-011-08): checked-state for the toggle.
    offline_mode: bool,
    locale: String,
    update_checks_enabled: bool,
}

/// Complete description of what the tray should look like.
#[derive(Clone, Debug, PartialEq, Eq)]
struct TrayDesired {
    icon_path: &'static str,
    menu: MenuInputs,
}

struct TrayInner {
    /// Intent set by [`set_tray_state`].
    icon_state: TrayIconState,
    /// Latest computed snapshot, waiting to be (or just) applied.
    desired: Option<TrayDesired>,
    /// Icon the native tray currently shows. Only updated when `set_icon`
    /// succeeds, so a failed update is retried on the next sync.
    applied_icon: Option<&'static str>,
    /// Inputs the native menu was last successfully built from. The tooltip
    /// is derived from the same inputs and set best-effort alongside the menu;
    /// it is not tracked separately.
    applied_menu: Option<MenuInputs>,
    /// An apply is scheduled on the main thread.
    pending: bool,
    /// Decoded icons by resource path so the main thread never touches disk.
    icons: HashMap<&'static str, Image<'static>>,
    /// Handed out to each sync request in trigger order, so a slow request
    /// can't overwrite the snapshot of one that was triggered after it.
    next_seq: u64,
    /// Sequence number of the request that produced `desired`.
    desired_seq: u64,
}

/// Tauri managed state owning the tray's desired/applied snapshots.
pub struct TrayState(Mutex<TrayInner>);

impl TrayState {
    pub fn new() -> Self {
        Self(Mutex::new(TrayInner {
            icon_state: TrayIconState::Idle,
            desired: None,
            applied_icon: None,
            applied_menu: None,
            pending: false,
            icons: HashMap::new(),
            next_seq: 0,
            desired_seq: 0,
        }))
    }

    fn lock(&self) -> MutexGuard<'_, TrayInner> {
        self.0.lock().unwrap_or_else(|poisoned| {
            warn!("Tray state mutex was poisoned, recovering");
            poisoned.into_inner()
        })
    }
}

impl Default for TrayState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum AppTheme {
    Dark,
    Light,
    Colored, // Pink/colored theme for Linux
}

/// Gets the current app theme, with Linux defaulting to Colored theme
pub fn get_current_theme(app: &AppHandle) -> AppTheme {
    if cfg!(target_os = "linux") {
        // On Linux, always use the colored theme
        AppTheme::Colored
    } else {
        // On Windows the tray icon sits on the taskbar, which follows the
        // *system* theme (SystemUsesLightTheme), not the app theme. With the
        // "Custom" personalization mode the two can differ (e.g. dark taskbar
        // + light apps), and the window theme would pick an icon that is
        // invisible against the taskbar.
        #[cfg(target_os = "windows")]
        if let Some(theme) = windows_taskbar_theme() {
            return theme;
        }

        // On other platforms, map system theme to our app theme
        if let Some(main_window) = app.get_webview_window(crate::window_labels::HUB) {
            match main_window.theme().unwrap_or(Theme::Dark) {
                Theme::Light => AppTheme::Light,
                Theme::Dark => AppTheme::Dark,
                _ => AppTheme::Dark, // Default fallback
            }
        } else {
            AppTheme::Dark
        }
    }
}

/// Reads the Windows taskbar theme from the registry.
///
/// Returns None if the value is missing (older Windows 10 builds default to a
/// dark taskbar there, but falling back to the window theme is safer than
/// guessing).
#[cfg(target_os = "windows")]
fn windows_taskbar_theme() -> Option<AppTheme> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let personalize = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize")
        .ok()?;
    let system_uses_light: u32 = personalize.get_value("SystemUsesLightTheme").ok()?;
    Some(if system_uses_light == 1 {
        AppTheme::Light
    } else {
        AppTheme::Dark
    })
}

/// Gets the appropriate icon path for the given theme and state.
///
/// `warning` overlays a badge on the idle icon while keyboard shortcuts are
/// blocked (macOS Secure Input); recording/transcribing states keep their
/// normal icons so in-flight activity stays recognizable.
pub fn get_icon_path(theme: AppTheme, state: TrayIconState, warning: bool) -> &'static str {
    if warning && state == TrayIconState::Idle {
        return match theme {
            AppTheme::Dark => "resources/tray_idle_warning.png",
            AppTheme::Light => "resources/tray_idle_warning_dark.png",
            // Linux never sets the warning flag (Secure Input is macOS-only),
            // but fall back to the normal icon just in case.
            AppTheme::Colored => "resources/idle.png",
        };
    }
    match (theme, state) {
        // Dark theme uses light icons
        (AppTheme::Dark, TrayIconState::Idle) => "resources/tray_idle.png",
        (AppTheme::Dark, TrayIconState::Recording) => "resources/tray_recording.png",
        (AppTheme::Dark, TrayIconState::Transcribing) => "resources/tray_transcribing.png",
        // Light theme uses dark icons
        (AppTheme::Light, TrayIconState::Idle) => "resources/tray_idle_dark.png",
        (AppTheme::Light, TrayIconState::Recording) => "resources/tray_recording_dark.png",
        (AppTheme::Light, TrayIconState::Transcribing) => "resources/tray_transcribing_dark.png",
        // Colored theme uses pink icons (for Linux)
        (AppTheme::Colored, TrayIconState::Idle) => "resources/idle.png",
        (AppTheme::Colored, TrayIconState::Recording) => "resources/recording.png",
        (AppTheme::Colored, TrayIconState::Transcribing) => "resources/transcribing.png",
    }
}

/// Sets the recording state shown by the tray (icon + Cancel/model menu).
pub fn set_tray_state(app: &AppHandle, state: TrayIconState) {
    sync_tray_with(app, |inner| inner.icon_state = state);
}

/// Re-syncs the tray after something other than the recording state changed
/// (theme, Secure Input warning). The recording state itself is preserved.
pub fn refresh_tray_icon(app: &AppHandle) {
    sync_tray(app);
}

/// Re-syncs the tray after something the menu depends on changed (model
/// list/selection/loaded state, language, settings).
pub fn update_tray_menu(app: &AppHandle) {
    sync_tray(app);
}

/// Records the current desired tray state and schedules one apply on the main
/// thread (or lets an already-pending apply pick it up). Never blocks on the
/// main thread.
///
/// The snapshot (settings, model list, loaded state) is computed on the
/// *calling* thread on purpose: the main-thread applier must not take manager
/// locks that a worker may hold across slow work (see #1716).
pub fn sync_tray(app: &AppHandle) {
    sync_tray_with(app, |_| {});
}

fn sync_tray_with(app: &AppHandle, update: impl FnOnce(&mut TrayInner)) {
    let Some(state) = app.try_state::<TrayState>() else {
        return;
    };

    // Record intent and claim a sequence number in one critical section, so
    // sequence order == the order in which state changes were requested.
    let (seq, icon_state) = {
        let mut inner = state.lock();
        update(&mut inner);
        inner.next_seq += 1;
        (inner.next_seq, inner.icon_state)
    };

    // Tray not built yet (early secure-input monitor callbacks). The intent
    // is kept and picked up by the first sync after the tray exists.
    if app.try_state::<TrayIcon>().is_none() {
        return;
    }

    let desired = compute_desired(app, icon_state);

    // Decode the icon off the main thread, once per path, outside the lock.
    let needs_icon = !state.lock().icons.contains_key(desired.icon_path);
    let loaded_icon = if needs_icon {
        match load_tray_icon(
            app.path()
                .resolve(desired.icon_path, tauri::path::BaseDirectory::Resource),
        ) {
            Ok(image) => Some(image),
            Err(err) => {
                error!("Failed to load tray icon '{}': {err}", desired.icon_path);
                None
            }
        }
    } else {
        None
    };

    let schedule = {
        let mut inner = state.lock();
        if let Some(image) = loaded_icon {
            inner.icons.insert(desired.icon_path, image);
        }
        if seq < inner.desired_seq {
            // A request triggered after this one already stored its snapshot
            // (and scheduled an apply). Ours is stale; drop it.
            trace!(
                "tray sync: request {seq} superseded by {}",
                inner.desired_seq
            );
            return;
        }
        inner.desired = Some(desired);
        inner.desired_seq = seq;
        // If an apply is already pending it will read the snapshot we just
        // stored; otherwise schedule one.
        !std::mem::replace(&mut inner.pending, true)
    };

    if schedule {
        post_apply(app);
    } else {
        trace!("tray sync: apply already pending");
    }
}

fn compute_desired(app: &AppHandle, icon_state: TrayIconState) -> TrayDesired {
    let settings = settings::get_settings(app);
    let theme = get_current_theme(app);
    let warning = crate::secure_input::tray_warning_active(app);
    // FR-009-07: a recording meeting shows the same red icon as a dictation
    // — audio is being captured either way. The *menu* flags below stay on
    // the dictation's own state ("Stop Dictation"/"Cancel" describe it).
    let meeting_active = crate::meeting::session::meeting_recording_active();
    let display_state = if meeting_active {
        TrayIconState::Recording
    } else {
        icon_state
    };

    TrayDesired {
        icon_path: get_icon_path(theme, display_state, warning),
        menu: MenuInputs {
            busy: icon_state.is_busy(),
            recording: icon_state == TrayIconState::Recording,
            warning,
            meeting_active,
            flowbar_hidden: crate::overlay::is_flowbar_user_hidden(),
            meeting_detection_paused: meeting_detection_is_paused(
                settings.meeting_detection_paused_until_ms,
                now_unix_ms(),
            ),
            offline_mode: settings.offline_mode,
            locale: settings.app_language,
            update_checks_enabled: settings.update_checks_enabled,
        },
    }
}

fn post_apply(app: &AppHandle) {
    let handle = app.clone();
    if let Err(err) = app.run_on_main_thread(move || apply_on_main(&handle)) {
        // Event loop is gone (shutdown). Clear `pending` so a later call, if
        // any, doesn't wait forever for an apply that will never run.
        error!("Failed to dispatch tray update to the main thread: {err}");
        if let Some(state) = app.try_state::<TrayState>() {
            state.lock().pending = false;
        }
    }
}

/// The single writer to the native tray. Runs on the main thread.
fn apply_on_main(app: &AppHandle) {
    let Some(state) = app.try_state::<TrayState>() else {
        return;
    };
    let Some(tray) = app.try_state::<TrayIcon>() else {
        return;
    };

    let started = Instant::now();
    let (desired, icon, icon_changed, menu_changed) = {
        let mut inner = state.lock();
        inner.pending = false;
        let Some(desired) = inner.desired.clone() else {
            return;
        };
        let icon_changed = inner.applied_icon != Some(desired.icon_path);
        let menu_changed = inner.applied_menu.as_ref() != Some(&desired.menu);
        if !icon_changed && !menu_changed {
            trace!("tray apply: nothing changed");
            return;
        }
        let icon = inner.icons.get(desired.icon_path).cloned();
        (desired, icon, icon_changed, menu_changed)
    };

    // Each part is recorded as applied only if its native call succeeded, so a
    // transient failure is retried on the next sync instead of being
    // remembered as displayed.
    let mut icon_ok = false;
    if icon_changed {
        match icon {
            Some(image) => match tray.set_icon_with_as_template(Some(image), true) {
                Ok(()) => icon_ok = true,
                Err(err) => error!("Failed to update tray icon '{}': {err}", desired.icon_path),
            },
            None => error!("Tray icon '{}' is not loaded", desired.icon_path),
        }
    }

    let mut menu_ok = false;
    if menu_changed {
        match build_menu(app, &desired.menu) {
            Ok((menu, tooltip)) => match tray.set_menu(Some(menu)) {
                Ok(()) => {
                    menu_ok = true;
                    // Best-effort: logged, not retried. The tooltip is cosmetic
                    // and can only fail on Windows, where a failing
                    // Shell_NotifyIcon call means the icon is failing too.
                    // Gating `menu_ok` on it would re-run the full menu
                    // rebuild on every sync for the cheapest mutation.
                    if let Err(err) = tray.set_tooltip(Some(tooltip)) {
                        error!("Failed to set tray tooltip: {err}");
                    }
                }
                Err(err) => error!("Failed to set tray menu: {err}"),
            },
            Err(err) => error!("Failed to build tray menu: {err}"),
        }
    }

    {
        let mut inner = state.lock();
        if icon_ok {
            inner.applied_icon = Some(desired.icon_path);
        }
        if menu_ok {
            inner.applied_menu = Some(desired.menu.clone());
        }
    }

    debug!(
        "tray apply: icon={} menu={} busy={} took={:?}",
        if icon_changed {
            desired.icon_path
        } else {
            "unchanged"
        },
        if menu_changed { "rebuilt" } else { "unchanged" },
        desired.menu.busy,
        started.elapsed()
    );
}

fn load_tray_icon(resolved_icon_path: tauri::Result<PathBuf>) -> tauri::Result<Image<'static>> {
    let resolved_icon_path = resolved_icon_path?;
    Image::from_path(&resolved_icon_path).map(Image::to_owned)
}

pub fn tray_tooltip() -> String {
    version_label()
}

fn version_label() -> String {
    if cfg!(debug_assertions) {
        format!("Transcreve.ai v{} (Dev)", env!("CARGO_PKG_VERSION"))
    } else {
        format!("Transcreve.ai v{}", env!("CARGO_PKG_VERSION"))
    }
}

/// Builds the tray menu and tooltip for the given inputs. Pure with respect
/// to app state: everything it depends on is in `inputs`, plus the
/// process-constant release-channel / `TRANSCREVE_DISABLE_UPDATER` state behind
/// `update_checks_forced_disabled()`, which cannot change during a run.
///
/// Menu layout follows FR-010-14: Abrir Hub · Iniciar/Parar ditado ·
/// Iniciar/Parar reunião · Mostrar/Ocultar Flow Bar · Pausar detecção de
/// reuniões por 1 h · Modo offline · Sair.
fn build_menu(app: &AppHandle, inputs: &MenuInputs) -> tauri::Result<(Menu<tauri::Wry>, String)> {
    let strings = get_tray_translations(Some(inputs.locale.clone()));

    // Secure Input warning entry (macOS): clicking opens the Hub window where
    // the full warning banner explains the situation. Locales that haven't
    // translated the key yet get the English string rather than a blank menu
    // item (build.rs emits "" for missing keys).
    let secure_input_warning = if inputs.warning {
        let label = if strings.secure_input_warning.is_empty() {
            get_tray_translations(Some("en".to_string())).secure_input_warning
        } else {
            strings.secure_input_warning.clone()
        };
        Some(MenuItem::with_id(
            app,
            "secure_input_warning",
            &label,
            true,
            None::<&str>,
        )?)
    } else {
        None
    };

    // Platform-specific accelerators
    #[cfg(target_os = "macos")]
    let (hub_accelerator, quit_accelerator) = (Some("Cmd+,"), Some("Cmd+Q"));
    #[cfg(not(target_os = "macos"))]
    let (hub_accelerator, quit_accelerator) = (Some("Ctrl+,"), Some("Ctrl+Q"));

    let open_hub_i = MenuItem::with_id(app, "open_hub", &strings.open_hub, true, hub_accelerator)?;

    let dictation_label = if inputs.recording {
        &strings.stop_dictation
    } else {
        &strings.start_dictation
    };
    let toggle_dictation_i =
        MenuItem::with_id(app, "toggle_dictation", dictation_label, true, None::<&str>)?;

    // FR-010-14: "Iniciar/Parar reunião" — wired to the T-064 session.
    let meeting_label = if inputs.meeting_active {
        &strings.stop_meeting
    } else {
        &strings.start_meeting
    };
    let toggle_meeting_i =
        MenuItem::with_id(app, "toggle_meeting", meeting_label, true, None::<&str>)?;

    let flowbar_label = if inputs.flowbar_hidden {
        &strings.show_flowbar
    } else {
        &strings.hide_flowbar
    };
    let toggle_flowbar_i =
        MenuItem::with_id(app, "toggle_flowbar", flowbar_label, true, None::<&str>)?;

    let pause_detection_i = CheckMenuItem::with_id(
        app,
        "pause_meeting_detection",
        &strings.pause_meeting_detection,
        true,
        inputs.meeting_detection_paused,
        None::<&str>,
    )?;
    let offline_mode_i = CheckMenuItem::with_id(
        app,
        "offline_mode",
        &strings.offline_mode,
        true,
        inputs.offline_mode,
        None::<&str>,
    )?;

    let check_updates_i = MenuItem::with_id(
        app,
        "check_updates",
        &strings.check_updates,
        inputs.update_checks_enabled,
        None::<&str>,
    )?;
    let quit_i = MenuItem::with_id(app, "quit", &strings.quit, true, quit_accelerator)?;

    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let sep3 = PredefinedMenuItem::separator(app)?;
    let sep4 = PredefinedMenuItem::separator(app)?;

    let mut items: Vec<&dyn IsMenuItem<tauri::Wry>> = vec![&open_hub_i, &sep1, &toggle_dictation_i];

    // While a dictation/transcription is in flight the menu gains a "Cancel"
    // row (discards the operation — the Escape binding's menu twin).
    let cancel_i;
    if inputs.busy {
        cancel_i = MenuItem::with_id(app, "cancel", &strings.cancel, true, None::<&str>)?;
        items.push(&cancel_i);
    }

    items.push(&toggle_meeting_i);
    items.push(&sep2);
    items.push(&toggle_flowbar_i);
    items.push(&pause_detection_i);
    items.push(&offline_mode_i);
    items.push(&sep3);
    items.push(&check_updates_i);
    items.push(&sep4);
    items.push(&quit_i);

    let menu = Menu::with_items(app, &items)?;

    // When update checks are forced off (no release channel in this build —
    // FR-010-19: no updater in v1 — or TRANSCREVE_DISABLE_UPDATER, set by the
    // Nix package), the item is dropped from the menu rather than shown
    // disabled — it can never do anything in that case, and a disabled item
    // still shifts every entry below it by one position. A manually-disabled
    // toggle in Debug Settings keeps the old greyed-out behavior via the
    // enabled flag.
    if settings::update_checks_forced_disabled() {
        menu.remove(&check_updates_i)?;
        // Its leading separator goes with it so two separators never end up
        // adjacent.
        menu.remove(&sep3)?;
    }

    // Slot the warning at the very top so it's the first thing seen.
    let mut tooltip = version_label();
    if inputs.offline_mode {
        tooltip = format!("{} — {}", tooltip, strings.offline_mode);
    }
    if let Some(warning_item) = secure_input_warning {
        menu.insert(&warning_item, 0)?;
        menu.insert(&PredefinedMenuItem::separator(app)?, 1)?;
        tooltip = format!("{} — {}", tooltip, warning_item.text().unwrap_or_default());
    }

    Ok((menu, tooltip))
}

/// Length of the tray's meeting-detection pause (FR-010-14 "por 1 h").
pub const MEETING_DETECTION_PAUSE_MS: i64 = 60 * 60 * 1000;

/// Current unix time in milliseconds (`0` if the clock is before the epoch).
pub fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Whether a `meeting_detection_paused_until_ms` deadline still lies ahead.
pub fn meeting_detection_is_paused(paused_until: Option<i64>, now_ms: i64) -> bool {
    paused_until.is_some_and(|until| until > now_ms)
}

/// Whether quitting now should be confirmed first (FR-010-15 / AC-010-06): a
/// dictation recording or a live meeting would be silently lost.
pub fn quit_needs_confirmation(app: &AppHandle) -> bool {
    app.try_state::<Arc<AudioRecordingManager>>()
        .is_some_and(|manager| manager.is_recording())
        || crate::meeting::session::meeting_recording_active()
}

pub fn set_tray_visibility(app: &AppHandle, visible: bool) {
    let tray = app.state::<TrayIcon>();
    if let Err(e) = tray.set_visible(visible) {
        error!("Failed to set tray visibility: {}", e);
    } else {
        info!("Tray visibility set to: {}", visible);
    }
}

/// Recovery for the macOS tray-disappearance bug (#1948, tauri-apps/tauri#12060):
/// the `NSStatusItem` can silently vanish with no error surfaced to the app.
/// Hiding and re-showing the tray recreates it with its current icon, menu and
/// tooltip. Called when the user "relaunches" Handy while it is already running
/// (`RunEvent::Reopen` for Spotlight/Finder/Dock, the single-instance callback
/// for a second process) — the natural "where did my icon go?" moment — so a
/// relaunch brings the icon back without a full quit.
#[cfg(target_os = "macos")]
pub fn recreate_tray_icon(app: &AppHandle) {
    let no_tray = app
        .try_state::<crate::cli::CliArgs>()
        .map(|args| args.no_tray)
        .unwrap_or(false);
    if no_tray || !settings::get_settings(app).show_tray_icon {
        return;
    }
    let Some(tray) = app.try_state::<TrayIcon>() else {
        return;
    };
    info!("Recreating tray icon on relaunch");
    if let Err(e) = tray.set_visible(false).and_then(|_| tray.set_visible(true)) {
        error!("Failed to recreate tray icon: {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        load_tray_icon, meeting_detection_is_paused, MenuInputs, TrayDesired,
        MEETING_DETECTION_PAUSE_MS,
    };

    fn inputs(busy: bool, recording: bool) -> MenuInputs {
        MenuInputs {
            busy,
            recording,
            warning: false,
            meeting_active: false,
            flowbar_hidden: false,
            meeting_detection_paused: false,
            offline_mode: false,
            locale: "en".to_string(),
            update_checks_enabled: true,
        }
    }

    #[test]
    fn tray_icon_resolution_failure_is_returned_instead_of_panicking() {
        assert!(load_tray_icon(Err(tauri::Error::UnknownPath)).is_err());
    }

    #[test]
    fn tray_icon_returns_err_when_file_does_not_exist() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let missing = dir.path().join("does_not_exist.png");
        assert!(load_tray_icon(Ok(missing)).is_err());
    }

    #[test]
    fn recording_and_transcribing_differ_only_in_label() {
        // The icon differs, and the dictation entry reads "Stop Dictation"
        // only while audio is actually being captured — a press during
        // Transcribing queues the *next* session, so it stays "Start".
        let recording = TrayDesired {
            icon_path: "resources/tray_recording.png",
            menu: MenuInputs {
                recording: true,
                ..inputs(true, false)
            },
        };
        let transcribing = TrayDesired {
            icon_path: "resources/tray_transcribing.png",
            menu: inputs(true, false),
        };
        assert!(recording.menu.busy && transcribing.menu.busy);
        assert_ne!(recording.icon_path, transcribing.icon_path);
        assert_ne!(recording.menu, transcribing.menu);
    }

    #[test]
    fn idle_and_busy_menus_differ() {
        assert_ne!(inputs(false, false), inputs(true, false));
    }

    #[test]
    fn meeting_detection_pause_deadline_semantics() {
        let now = 1_000_000i64;
        assert!(!meeting_detection_is_paused(None, now));
        assert!(!meeting_detection_is_paused(Some(now), now));
        assert!(!meeting_detection_is_paused(Some(now - 1), now));
        assert!(meeting_detection_is_paused(Some(now + 1), now));
        assert!(meeting_detection_is_paused(
            Some(now + MEETING_DETECTION_PAUSE_MS),
            now
        ));
    }
}
