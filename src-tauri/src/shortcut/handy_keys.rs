//! Handy-keys based keyboard shortcut implementation
//!
//! This module provides an alternative to Tauri's global-shortcut plugin
//! using the handy-keys library for more control over keyboard events.
//!
//! ## Architecture
//!
//! The implementation uses a dedicated manager thread that owns the
//! `KeyboardListener` (whose OS hook thread produces raw `KeyEvent`s) and
//! the pure `HotkeyMatcher` that classifies them into actions:
//!
//! ```text
//! ┌─────────────────┐     commands      ┌──────────────────────┐
//! │   Main Thread   │ ───────────────▶ │   Manager Thread     │
//! │                 │   (via channel)   │                      │
//! │ - register()    │                   │ - owns the listener │
//! │ - unregister()  │                   │ - feeds the matcher  │
//! └─────────────────┘                   │ - dispatches actions │
//!                                       └──────────────────────┘
//! ```
//!
//! Keeping the raw-event consumption and matching on a single thread avoids
//! locks; the hook callback itself stays minimal (classify + channel send,
//! inside the crate), while the matching logic lives in our code as the pure
//! `HotkeyMatcher` — the seam spec F002 requires for the `feed(event, now)`
//! matcher (T-021). Suppression of matched combos happens synchronously
//! inside the hook via the `BlockingHotkeys` set shared with the listener.
//!
//! Commands (register/unregister) are sent via an mpsc channel and
//! responses are synchronously awaited, so listener/matcher state is only
//! ever touched by the manager thread.
//!
//! ## Watchdog
//!
//! handy-keys re-arms the OS hooks on session unlock/console connect and on
//! resume from sleep (Windows can silently drop low-level hooks there). On
//! top of that, the manager thread detects a dead listener — a channel
//! disconnect means the hook thread exited — and re-creates the
//! `KeyboardListener` with exponential backoff, which reinstalls the hooks
//! and is logged. A hook silently removed by the OS while the thread stays
//! alive is not observable from user space and remains a known gap.
//!
//! ## Recording Mode
//!
//! For UI key capture, a separate `KeyboardListener` is created on-demand and
//! polled from a dedicated recording thread. Events are emitted to the frontend
//! via Tauri's event system.

use handy_keys::{BlockingHotkeys, Hotkey, KeyboardListener};
use log::{debug, error, info, warn};
use serde::Serialize;
use specta::Type;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

use crate::settings::{self, get_settings, ShortcutBinding};

use super::handler::handle_shortcut_event;
use super::matcher::{HotkeyAction, HotkeyMatcher};

/// Commands that can be sent to the hotkey manager thread
enum ManagerCommand {
    Register {
        binding_id: String,
        hotkey_string: String,
        response: Sender<Result<(), String>>,
    },
    Unregister {
        binding_id: String,
        response: Sender<Result<(), String>>,
    },
    Shutdown,
}

/// State for the handy-keys shortcut manager
pub struct HandyKeysState {
    /// Channel to send commands to the manager thread (wrapped in Mutex for Sync)
    command_sender: Mutex<Sender<ManagerCommand>>,
    /// Handle to the manager thread (wrapped in Mutex for Sync, allows proper join on drop)
    thread_handle: Mutex<Option<JoinHandle<()>>>,
    /// Recording listener for UI key capture (only active during recording)
    recording_listener: Mutex<Option<KeyboardListener>>,
    /// Flag indicating if we're in recording mode
    is_recording: AtomicBool,
    /// The binding ID being recorded (if any)
    recording_binding_id: Mutex<Option<String>>,
    /// Flag to stop recording loop
    recording_running: Arc<AtomicBool>,
}

/// Key event sent to frontend during recording mode
#[derive(Debug, Clone, Serialize, Type)]
pub struct FrontendKeyEvent {
    /// Currently pressed modifier keys
    pub modifiers: Vec<String>,
    /// The key that was pressed (if any)
    pub key: Option<String>,
    /// Whether this is a key down event
    pub is_key_down: bool,
    /// The full hotkey string (e.g., "option+space")
    pub hotkey_string: String,
}

/// Cap for the listener-respawn backoff in the manager thread.
const RESTART_BACKOFF_MAX: Duration = Duration::from_secs(5);
/// Initial delay before respawning a dead listener.
const RESTART_BACKOFF_INIT: Duration = Duration::from_millis(100);

impl HandyKeysState {
    /// Create a new HandyKeysState
    pub fn new(app: AppHandle) -> Result<Self, String> {
        let (cmd_tx, cmd_rx) = mpsc::channel::<ManagerCommand>();

        // Hotkeys the hook suppresses synchronously; shared with the
        // listener and updated by the manager thread on register/unregister.
        let blocking_hotkeys: BlockingHotkeys = Arc::new(Mutex::new(HashSet::new()));

        // Install the listener (and its OS hook thread) up-front: a failure
        // here — e.g. missing accessibility permission on macOS — surfaces
        // to init_shortcuts so the caller can fall back to the Tauri
        // implementation instead of running with dead shortcuts.
        let listener = KeyboardListener::new_with_blocking(Arc::clone(&blocking_hotkeys))
            .map_err(|e| format!("Failed to install keyboard hook: {}", e))?;

        // Start the manager thread
        let app_clone = app.clone();
        let thread_handle = thread::spawn(move || {
            Self::manager_thread(cmd_rx, app_clone, listener, blocking_hotkeys);
        });

        Ok(Self {
            command_sender: Mutex::new(cmd_tx),
            thread_handle: Mutex::new(Some(thread_handle)),
            recording_listener: Mutex::new(None),
            is_recording: AtomicBool::new(false),
            recording_binding_id: Mutex::new(None),
            recording_running: Arc::new(AtomicBool::new(false)),
        })
    }

    /// The main manager thread - owns the KeyboardListener and the pure
    /// HotkeyMatcher, and processes commands.
    fn manager_thread(
        cmd_rx: Receiver<ManagerCommand>,
        app: AppHandle,
        listener: KeyboardListener,
        blocking_hotkeys: BlockingHotkeys,
    ) {
        info!("handy-keys manager thread started");

        let mut listener = Some(listener);
        let mut matcher = HotkeyMatcher::new();
        // Watchdog state: when the listener's event channel disconnects its
        // hook thread has died, so we respawn it (reinstalling the OS hooks)
        // with exponential backoff.
        let mut restart_backoff = RESTART_BACKOFF_INIT;
        let mut next_restart = Instant::now();

        loop {
            // Drain raw key events into the matcher and dispatch actions.
            let mut listener_dead = false;
            if let Some(l) = &listener {
                loop {
                    // A zero timeout behaves like try_recv but keeps the
                    // Timeout/Disconnected distinction that try_recv loses.
                    match l.recv_timeout(Duration::from_millis(0)) {
                        Ok(event) => {
                            for action in matcher.feed(&event, Instant::now()) {
                                Self::dispatch(&app, &matcher, action);
                            }
                        }
                        Err(handy_keys::Error::Timeout) => break,
                        Err(e) => {
                            error!("handy-keys event stream ended: {}", e);
                            listener_dead = true;
                            break;
                        }
                    }
                }
            }
            if listener_dead {
                // Dropping the listener joins its hook thread (already dead,
                // so this is instant). The shared blocking set survives, so
                // suppression resumes unchanged once the listener is back.
                listener = None;
                next_restart = Instant::now();
            }

            // Watchdog: respawn a dead listener. Re-creating it runs the
            // platform spawn path again, reinstalling the OS-level hooks.
            if listener.is_none() && Instant::now() >= next_restart {
                match KeyboardListener::new_with_blocking(Arc::clone(&blocking_hotkeys)) {
                    Ok(l) => {
                        warn!("handy-keys: keyboard listener restarted after hook thread death");
                        listener = Some(l);
                        restart_backoff = RESTART_BACKOFF_INIT;
                    }
                    Err(e) => {
                        error!(
                            "handy-keys: failed to restart keyboard listener (retry in {:?}): {}",
                            restart_backoff, e
                        );
                        next_restart = Instant::now() + restart_backoff;
                        restart_backoff = (restart_backoff * 2).min(RESTART_BACKOFF_MAX);
                    }
                }
            }

            // Check for commands (non-blocking with timeout)
            match cmd_rx.recv_timeout(Duration::from_millis(10)) {
                Ok(cmd) => match cmd {
                    ManagerCommand::Register {
                        binding_id,
                        hotkey_string,
                        response,
                    } => {
                        let result = Self::do_register(
                            &mut matcher,
                            &blocking_hotkeys,
                            &binding_id,
                            &hotkey_string,
                        );
                        let _ = response.send(result);
                    }
                    ManagerCommand::Unregister {
                        binding_id,
                        response,
                    } => {
                        let result =
                            Self::do_unregister(&mut matcher, &blocking_hotkeys, &binding_id);
                        let _ = response.send(result);
                    }
                    ManagerCommand::Shutdown => {
                        info!("handy-keys manager thread shutting down");
                        break;
                    }
                },
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // No command, continue
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    info!("Command channel disconnected, shutting down");
                    break;
                }
            }
        }

        info!("handy-keys manager thread stopped");
    }

    /// Dispatch one matcher action to the shared shortcut handler.
    fn dispatch(app: &AppHandle, matcher: &HotkeyMatcher, action: HotkeyAction) {
        let binding_id = action.binding_id();
        let is_pressed = matches!(action, HotkeyAction::Pressed(_));
        let hotkey_string = matcher.hotkey_string(binding_id).unwrap_or_default();
        debug!(
            "handy-keys event: binding={}, hotkey={}, pressed={}",
            binding_id, hotkey_string, is_pressed
        );
        handle_shortcut_event(app, binding_id, hotkey_string, is_pressed);
    }

    /// Register a hotkey in the matcher and the hook's suppression set
    fn do_register(
        matcher: &mut HotkeyMatcher,
        blocking_hotkeys: &BlockingHotkeys,
        binding_id: &str,
        hotkey_string: &str,
    ) -> Result<(), String> {
        let hotkey: Hotkey = hotkey_string
            .parse()
            .map_err(|e| format!("Failed to parse hotkey '{}': {}", hotkey_string, e))?;

        let previous = matcher
            .register(binding_id, hotkey, hotkey_string.to_string())
            .map_err(|e| format!("Failed to register hotkey: {}", e))?;

        match blocking_hotkeys.lock() {
            Ok(mut set) => {
                if let Some(old) = previous {
                    set.remove(&old);
                }
                set.insert(hotkey);
            }
            Err(_) => {
                error!(
                    "handy-keys: suppression set lock poisoned; '{}' will fire but not be suppressed",
                    binding_id
                );
            }
        }

        debug!(
            "Registered handy-keys shortcut: {} -> {:?}",
            binding_id, hotkey
        );
        Ok(())
    }

    /// Unregister a hotkey from the matcher and the suppression set
    fn do_unregister(
        matcher: &mut HotkeyMatcher,
        blocking_hotkeys: &BlockingHotkeys,
        binding_id: &str,
    ) -> Result<(), String> {
        if let Some(hotkey) = matcher.unregister(binding_id) {
            match blocking_hotkeys.lock() {
                Ok(mut set) => {
                    set.remove(&hotkey);
                }
                Err(_) => {
                    error!(
                        "handy-keys: suppression set lock poisoned; '{}' may keep being suppressed",
                        binding_id
                    );
                }
            }
            debug!("Unregistered handy-keys shortcut: {}", binding_id);
        }
        Ok(())
    }

    /// Register a shortcut binding
    pub fn register(&self, binding: &ShortcutBinding) -> Result<(), String> {
        let (tx, rx) = mpsc::channel();
        self.command_sender
            .lock()
            .map_err(|_| "Failed to lock command_sender")?
            .send(ManagerCommand::Register {
                binding_id: binding.id.clone(),
                hotkey_string: binding.current_binding.clone(),
                response: tx,
            })
            .map_err(|_| "Failed to send register command")?;

        rx.recv()
            .map_err(|_| "Failed to receive register response")?
    }

    /// Unregister a shortcut binding
    pub fn unregister(&self, binding: &ShortcutBinding) -> Result<(), String> {
        let (tx, rx) = mpsc::channel();
        self.command_sender
            .lock()
            .map_err(|_| "Failed to lock command_sender")?
            .send(ManagerCommand::Unregister {
                binding_id: binding.id.clone(),
                response: tx,
            })
            .map_err(|_| "Failed to send unregister command")?;

        rx.recv()
            .map_err(|_| "Failed to receive unregister response")?
    }

    /// Start recording mode for a specific binding
    pub fn start_recording(&self, app: &AppHandle, binding_id: String) -> Result<(), String> {
        if self.is_recording.load(Ordering::SeqCst) {
            return Err("Already recording".into());
        }

        // Create a new keyboard listener for recording
        let listener = KeyboardListener::new()
            .map_err(|e| format!("Failed to create keyboard listener: {}", e))?;

        {
            let mut recording = self
                .recording_listener
                .lock()
                .map_err(|_| "Failed to lock recording_listener")?;
            *recording = Some(listener);
        }
        {
            let mut binding = self
                .recording_binding_id
                .lock()
                .map_err(|_| "Failed to lock recording_binding_id")?;
            *binding = Some(binding_id);
        }

        self.is_recording.store(true, Ordering::SeqCst);
        self.recording_running.store(true, Ordering::SeqCst);

        // Start a thread to emit key events to the frontend
        let app_clone = app.clone();
        let recording_running = Arc::clone(&self.recording_running);
        thread::spawn(move || {
            Self::recording_loop(app_clone, recording_running);
        });

        debug!("Started handy-keys recording mode");
        Ok(())
    }

    /// Recording loop - emits key events to frontend during recording
    fn recording_loop(app: AppHandle, running: Arc<AtomicBool>) {
        while running.load(Ordering::SeqCst) {
            let event = {
                let state = match app.try_state::<HandyKeysState>() {
                    Some(s) => s,
                    None => break,
                };
                let listener = state.recording_listener.lock().ok();
                listener.as_ref().and_then(|l| l.as_ref()?.try_recv())
            };

            if let Some(key_event) = event {
                // Convert to frontend-friendly format
                let frontend_event = FrontendKeyEvent {
                    modifiers: modifiers_to_strings(key_event.modifiers),
                    key: key_event.key.map(|k| k.to_string().to_lowercase()),
                    is_key_down: key_event.is_key_down,
                    hotkey_string: key_event
                        .as_hotkey()
                        .map(|h| h.to_handy_string())
                        .unwrap_or_default(),
                };

                // Emit to frontend
                if let Err(e) = app.emit("handy-keys-event", &frontend_event) {
                    error!("Failed to emit key event: {}", e);
                }
            } else {
                thread::sleep(std::time::Duration::from_millis(10));
            }
        }

        debug!("Recording loop ended");
    }

    /// Stop recording mode
    pub fn stop_recording(&self) -> Result<(), String> {
        self.is_recording.store(false, Ordering::SeqCst);
        self.recording_running.store(false, Ordering::SeqCst);

        {
            let mut recording = self
                .recording_listener
                .lock()
                .map_err(|_| "Failed to lock recording_listener")?;
            *recording = None;
        }
        {
            let mut binding = self
                .recording_binding_id
                .lock()
                .map_err(|_| "Failed to lock recording_binding_id")?;
            *binding = None;
        }

        debug!("Stopped handy-keys recording mode");
        Ok(())
    }
}

impl Drop for HandyKeysState {
    fn drop(&mut self) {
        // Signal recording to stop
        self.recording_running.store(false, Ordering::SeqCst);
        self.is_recording.store(false, Ordering::SeqCst);

        // Send shutdown command
        if let Ok(sender) = self.command_sender.lock() {
            let _ = sender.send(ManagerCommand::Shutdown);
        }

        // Wait for the manager thread to finish
        if let Ok(mut handle) = self.thread_handle.lock() {
            if let Some(h) = handle.take() {
                let _ = h.join();
            }
        }
    }
}

/// Convert handy-keys Modifiers to a list of strings
fn modifiers_to_strings(modifiers: handy_keys::Modifiers) -> Vec<String> {
    let mut result = Vec::new();

    if modifiers.contains(handy_keys::Modifiers::CTRL) {
        result.push("ctrl".to_string());
    }
    if modifiers.contains(handy_keys::Modifiers::OPT) {
        #[cfg(target_os = "macos")]
        result.push("option".to_string());
        #[cfg(not(target_os = "macos"))]
        result.push("alt".to_string());
    }
    if modifiers.contains(handy_keys::Modifiers::SHIFT) {
        result.push("shift".to_string());
    }
    if modifiers.contains(handy_keys::Modifiers::CMD) {
        #[cfg(target_os = "macos")]
        result.push("command".to_string());
        #[cfg(not(target_os = "macos"))]
        result.push("super".to_string());
    }
    if modifiers.contains(handy_keys::Modifiers::FN) {
        result.push("fn".to_string());
    }

    result
}

/// Validate a shortcut string for the HandyKeys implementation.
/// HandyKeys is more permissive: allows modifier-only combos and the fn key.
pub fn validate_shortcut(raw: &str) -> Result<(), String> {
    if raw.trim().is_empty() {
        return Err("Shortcut cannot be empty".into());
    }
    // HandyKeys accepts modifier-only, key-only, and modifier+key combos
    // Just verify the string is parseable
    raw.parse::<Hotkey>()
        .map(|_| ())
        .map_err(|e| format!("Invalid shortcut for HandyKeys: {}", e))
}

/// Initialize handy-keys shortcuts
pub fn init_shortcuts(app: &AppHandle) -> Result<(), String> {
    let state = HandyKeysState::new(app.clone())?;

    let default_bindings = settings::get_default_settings().bindings;
    let user_settings = settings::load_or_create_app_settings(app);

    // Register all bindings except cancel (which is dynamic)
    for (id, default_binding) in default_bindings {
        if id == "cancel" {
            continue;
        }
        // Skip post-processing shortcut when the feature is disabled
        if id == "transcribe_with_post_process" && !user_settings.post_process_enabled {
            continue;
        }

        let binding = user_settings
            .bindings
            .get(&id)
            .cloned()
            .unwrap_or(default_binding);

        if let Err(e) = state.register(&binding) {
            error!(
                "Failed to register handy-keys shortcut {} during init: {}",
                id, e
            );
        }
    }

    app.manage(state);
    info!("handy-keys shortcuts initialized");
    Ok(())
}

/// Register the cancel shortcut (called when recording starts)
pub fn register_cancel_shortcut(app: &AppHandle) {
    // Disabled on Linux due to instability
    #[cfg(target_os = "linux")]
    {
        let _ = app;
        return;
    }

    #[cfg(not(target_os = "linux"))]
    {
        let app_clone = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Some(cancel_binding) = get_settings(&app_clone).bindings.get("cancel").cloned() {
                if let Some(state) = app_clone.try_state::<HandyKeysState>() {
                    if let Err(e) = state.register(&cancel_binding) {
                        error!("Failed to register cancel shortcut: {}", e);
                    }
                }
            }
        });
    }
}

/// Unregister the cancel shortcut (called when recording stops)
pub fn unregister_cancel_shortcut(app: &AppHandle) {
    #[cfg(target_os = "linux")]
    {
        let _ = app;
        return;
    }

    #[cfg(not(target_os = "linux"))]
    {
        let app_clone = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Some(cancel_binding) = get_settings(&app_clone).bindings.get("cancel").cloned() {
                if let Some(state) = app_clone.try_state::<HandyKeysState>() {
                    let _ = state.unregister(&cancel_binding);
                }
            }
        });
    }
}

/// Register a shortcut
pub fn register_shortcut(app: &AppHandle, binding: ShortcutBinding) -> Result<(), String> {
    let state = app
        .try_state::<HandyKeysState>()
        .ok_or("HandyKeysState not initialized")?;
    state.register(&binding)
}

/// Unregister a shortcut
pub fn unregister_shortcut(app: &AppHandle, binding: ShortcutBinding) -> Result<(), String> {
    let state = app
        .try_state::<HandyKeysState>()
        .ok_or("HandyKeysState not initialized")?;
    state.unregister(&binding)
}

/// Start key recording mode
#[tauri::command]
#[specta::specta]
pub fn start_handy_keys_recording(app: AppHandle, binding_id: String) -> Result<(), String> {
    let settings = get_settings(&app);
    if settings.keyboard_implementation != settings::KeyboardImplementation::HandyKeys {
        return Err("handy-keys is not the active keyboard implementation".into());
    }

    // While Secure Input is active the tap receives no KeyDown/KeyUp, so the
    // recorder would silently capture just the modifier and overwrite the
    // binding with it (issue #1578). Refuse instead; the frontend maps this
    // marker to a localized explanation, and the noted impact makes the
    // warning banner appear with the full story.
    if crate::secure_input::is_enabled_now() {
        crate::secure_input::note_recorder_blocked(&app);
        return Err("secure-input-active".into());
    }

    let state = app
        .try_state::<HandyKeysState>()
        .ok_or("HandyKeysState not initialized")?;

    // Suspend every registered shortcut so a combo that overlaps an existing
    // binding can't fire it (or have its keys swallowed) mid-capture.
    super::suspend_all_shortcuts(&app);

    let result = state.start_recording(&app, binding_id);
    if result.is_err() {
        super::resume_all_shortcuts(&app);
    }
    result
}

/// Stop key recording mode
#[tauri::command]
#[specta::specta]
pub fn stop_handy_keys_recording(app: AppHandle) -> Result<(), String> {
    let settings = get_settings(&app);
    if settings.keyboard_implementation != settings::KeyboardImplementation::HandyKeys {
        return Err("handy-keys is not the active keyboard implementation".into());
    }

    let state = app
        .try_state::<HandyKeysState>()
        .ok_or("HandyKeysState not initialized")?;

    // Restore shortcuts from settings regardless of how recording ended.
    // A commit has already registered the new binding via change_binding;
    // re-registering it here fails cleanly and is ignored.
    let result = state.stop_recording();
    super::resume_all_shortcuts(&app);
    result
}
