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

use crate::commands::{CommandError, CommandErrorCode, CommandResult};
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
/// How long a register/unregister call may wait for the manager thread.
const COMMAND_RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);
/// Hard cap on UI key-capture mode so a wedged recording session can't keep
/// broadcasting keystrokes (or leave every shortcut suspended) forever.
const RECORDING_TIMEOUT: Duration = Duration::from_secs(30);
/// Event emitted once when the watchdog gives up restarting the hook.
const HOOK_DEAD_EVENT: &str = "shortcut://hook-dead";

/// Exponential-backoff policy for respawning the keyboard listener after its
/// hook thread dies or a respawn fails. Pure and clock-free: callers report
/// how long the just-dead listener had been alive.
///
/// Backing off on *deaths* — not only on spawn failures — matters on
/// Windows, where `new_with_blocking` returns `Ok` even when the hook could
/// not be installed: the hook thread dies right away, and without a growing
/// delay the watchdog would re-create the listener every poll cycle.
struct RestartBackoff {
    consecutive_failures: u32,
}

impl RestartBackoff {
    /// Consecutive failures after which the watchdog stops trying.
    const MAX_FAILURES: u32 = 10;
    /// Uptime that marks a listener as stable; a death after it first resets
    /// the failure count so a fresh failure restarts quickly.
    const STABLE_UPTIME: Duration = Duration::from_secs(30);

    fn new() -> Self {
        Self {
            consecutive_failures: 0,
        }
    }

    /// Record a listener death or a failed respawn and return the delay
    /// before the next attempt — `None` once the failure cap is reached.
    /// `alive_for` is how long the just-dead listener ran (`None` for a
    /// spawn failure); a stable run resets the count first.
    fn next_delay(&mut self, alive_for: Option<Duration>) -> Option<Duration> {
        if alive_for.is_some_and(|uptime| uptime >= Self::STABLE_UPTIME) {
            self.consecutive_failures = 0;
        }
        self.consecutive_failures += 1;
        if self.consecutive_failures >= Self::MAX_FAILURES {
            return None;
        }
        let delay = RESTART_BACKOFF_INIT * 2u32.saturating_pow(self.consecutive_failures - 1);
        Some(delay.min(RESTART_BACKOFF_MAX))
    }
}

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
        let mut listener_alive_since = Instant::now();
        let mut matcher = HotkeyMatcher::new();
        // Watchdog state: when the listener's event channel disconnects its
        // hook thread has died, so we respawn it (reinstalling the OS hooks)
        // under RestartBackoff — which also gives up after too many
        // consecutive failures.
        let mut backoff = RestartBackoff::new();
        let mut next_restart = Instant::now();
        let mut hook_gave_up = false;

        loop {
            // Drain raw key events into the matcher and dispatch actions.
            let mut listener_dead = false;
            if let Some(l) = &listener {
                loop {
                    // A zero timeout behaves like try_recv but keeps the
                    // Timeout/Disconnected distinction that try_recv loses.
                    match l.recv_timeout(Duration::from_millis(0)) {
                        Ok(event) => {
                            // Drop events while we inject a paste/submit
                            // chord ourselves: they are our own synthetics,
                            // which must never perturb matcher state (e.g.
                            // an injected key-up releasing a held binding).
                            if crate::input::is_injection_active() {
                                continue;
                            }
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
                // Any binding still held loses its release edge with the
                // dead stream — emit it so push-to-talk can't stay stuck on.
                for action in matcher.release_all() {
                    Self::dispatch(&app, &matcher, action);
                }
                match backoff.next_delay(Some(listener_alive_since.elapsed())) {
                    Some(delay) => {
                        warn!(
                            "handy-keys: hook thread died; restarting listener in {:?}",
                            delay
                        );
                        next_restart = Instant::now() + delay;
                    }
                    None => {
                        Self::notify_hook_dead(&app);
                        hook_gave_up = true;
                    }
                }
            }

            // Watchdog: respawn a dead listener. Re-creating it runs the
            // platform spawn path again, reinstalling the OS-level hooks.
            // The backoff only resets once a listener survives past
            // RestartBackoff::STABLE_UPTIME — a respawn that dies quickly
            // keeps backing off instead of looping tightly.
            if listener.is_none() && !hook_gave_up && Instant::now() >= next_restart {
                match KeyboardListener::new_with_blocking(Arc::clone(&blocking_hotkeys)) {
                    Ok(l) => {
                        warn!("handy-keys: keyboard listener restarted after hook thread death");
                        listener = Some(l);
                        listener_alive_since = Instant::now();
                    }
                    Err(e) => match backoff.next_delay(None) {
                        Some(delay) => {
                            error!(
                                "handy-keys: failed to restart keyboard listener (retry in {:?}): {}",
                                delay, e
                            );
                            next_restart = Instant::now() + delay;
                        }
                        None => {
                            Self::notify_hook_dead(&app);
                            hook_gave_up = true;
                        }
                    },
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
                            &app,
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
                            Self::do_unregister(&app, &mut matcher, &blocking_hotkeys, &binding_id);
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

    /// The hook is unrecoverable: log loudly and notify the UI once. If a
    /// fallback implementation is registered the matcher keeps working there.
    fn notify_hook_dead(app: &AppHandle) {
        error!(
            "handy-keys: keyboard hook failed {} times in a row; giving up on restart",
            RestartBackoff::MAX_FAILURES
        );
        if let Err(e) = app.emit(HOOK_DEAD_EVENT, ()) {
            warn!("handy-keys: failed to emit {}: {}", HOOK_DEAD_EVENT, e);
        }
    }

    /// Dispatch one matcher action to the shared shortcut handler.
    fn dispatch(app: &AppHandle, matcher: &HotkeyMatcher, action: HotkeyAction) {
        let binding_id = action.binding_id().to_string();
        let is_pressed = matches!(action, HotkeyAction::Pressed(_));
        let hotkey_string = matcher
            .hotkey_string(&binding_id)
            .unwrap_or_default()
            .to_string();
        debug!(
            "handy-keys event: binding={}, hotkey={}, pressed={}",
            binding_id, hotkey_string, is_pressed
        );
        // A panicking handler must not take the manager thread down with it:
        // nothing supervises or respawns it.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            handle_shortcut_event(app, &binding_id, &hotkey_string, is_pressed);
        }));
        if result.is_err() {
            error!(
                "handy-keys: shortcut handler panicked for binding '{}'",
                binding_id
            );
        }
    }

    /// Register a hotkey in the matcher and the hook's suppression set
    fn do_register(
        app: &AppHandle,
        matcher: &mut HotkeyMatcher,
        blocking_hotkeys: &BlockingHotkeys,
        binding_id: &str,
        hotkey_string: &str,
    ) -> Result<(), String> {
        let hotkey: Hotkey = hotkey_string
            .parse()
            .map_err(|e| format!("Failed to parse hotkey '{}': {}", hotkey_string, e))?;

        let mut set = blocking_hotkeys.lock().map_err(|_| {
            format!("handy-keys: suppression set lock poisoned registering '{binding_id}'")
        })?;

        // Re-registering a held binding discards its hold state — emit the
        // release edge instead of silently dropping it.
        if let Some(action) = matcher.release_binding(binding_id) {
            Self::dispatch(app, matcher, action);
        }

        let previous = matcher
            .register(binding_id, hotkey, hotkey_string.to_string())
            .map_err(|e| format!("Failed to register hotkey: {}", e))?;
        if let Some(old) = previous {
            set.remove(&old);
        }
        set.insert(hotkey);

        debug!(
            "Registered handy-keys shortcut: {} -> {:?}",
            binding_id, hotkey
        );
        Ok(())
    }

    /// Unregister a hotkey from the matcher and the suppression set
    fn do_unregister(
        app: &AppHandle,
        matcher: &mut HotkeyMatcher,
        blocking_hotkeys: &BlockingHotkeys,
        binding_id: &str,
    ) -> Result<(), String> {
        // A held binding loses its release edge on unregister — emit it
        // first, while dispatch can still look up its hotkey string.
        if let Some(action) = matcher.release_binding(binding_id) {
            Self::dispatch(app, matcher, action);
        }
        if let Some(hotkey) = matcher.unregister(binding_id) {
            let mut set = blocking_hotkeys.lock().map_err(|_| {
                format!("handy-keys: suppression set lock poisoned unregistering '{binding_id}'")
            })?;
            set.remove(&hotkey);
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

        rx.recv_timeout(COMMAND_RESPONSE_TIMEOUT)
            .map_err(|e| format!("Failed to receive register response: {}", e))?
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

        rx.recv_timeout(COMMAND_RESPONSE_TIMEOUT)
            .map_err(|e| format!("Failed to receive unregister response: {}", e))?
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
        let started = Instant::now();
        while running.load(Ordering::SeqCst) {
            // Safety net: a capture session abandoned by the frontend must
            // not keep broadcasting keystrokes — or leave every shortcut
            // suspended — forever.
            if started.elapsed() >= RECORDING_TIMEOUT {
                info!(
                    "handy-keys: key recording timed out after {:?}; stopping",
                    RECORDING_TIMEOUT
                );
                if let Some(state) = app.try_state::<HandyKeysState>() {
                    let _ = state.stop_recording();
                }
                super::resume_all_shortcuts(&app);
                break;
            }

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

                // Emit only to the hub window, which hosts the capture UI.
                if let Err(e) = app.emit_to(
                    crate::window_labels::HUB,
                    "handy-keys-event",
                    &frontend_event,
                ) {
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

/// Keystroke shapes the app injects itself when pasting a transcript or
/// auto-submitting (see `crate::input`). Each entry is one event state an
/// injection produces — every modifier press plus the final chord — and a
/// binding matching any of them would fire off our own synthetic input,
/// potentially re-triggering dictation or releasing a hold early. The
/// injected events carry no marker the hook could filter on, so such
/// bindings are rejected at validation time instead.
fn injected_paste_states() -> [Hotkey; 6] {
    use handy_keys::{Key, Modifiers};

    // input.rs injects Cmd on macOS, Ctrl elsewhere.
    #[cfg(target_os = "macos")]
    let primary = Modifiers::CMD;
    #[cfg(not(target_os = "macos"))]
    let primary = Modifiers::CTRL;

    [
        // Ctrl+V (Cmd+V on macOS): modifier press, then the chord.
        Hotkey {
            modifiers: primary,
            key: None,
        },
        Hotkey {
            modifiers: primary,
            key: Some(Key::V),
        },
        // Ctrl+Shift+V: Shift added, then the chord.
        Hotkey {
            modifiers: primary | Modifiers::SHIFT,
            key: None,
        },
        Hotkey {
            modifiers: primary | Modifiers::SHIFT,
            key: Some(Key::V),
        },
        // Shift+Insert.
        Hotkey {
            modifiers: Modifiers::SHIFT,
            key: None,
        },
        Hotkey {
            modifiers: Modifiers::SHIFT,
            key: Some(Key::Insert),
        },
    ]
}

/// Validate a shortcut string for the HandyKeys implementation.
/// HandyKeys is more permissive: allows modifier-only combos and the fn key.
pub fn validate_shortcut(raw: &str) -> Result<(), String> {
    if raw.trim().is_empty() {
        return Err("Shortcut cannot be empty".into());
    }
    // HandyKeys accepts modifier-only, key-only, and modifier+key combos
    // Just verify the string is parseable
    let hotkey: Hotkey = raw
        .parse()
        .map_err(|e| format!("Invalid shortcut for HandyKeys: {}", e))?;

    // Reject bindings that match any state of our own paste injection —
    // those keys come back through the hook indistinguishable from real
    // input (see `injected_paste_states`).
    if injected_paste_states()
        .iter()
        .any(|injected| hotkey.key == injected.key && hotkey.modifiers.matches(injected.modifiers))
    {
        return Err(format!(
            "Shortcut '{raw}' is reserved: it collides with a keystroke Transcreve.ai injects itself when pasting transcripts"
        ));
    }
    Ok(())
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
pub fn start_handy_keys_recording(app: AppHandle, binding_id: String) -> CommandResult<()> {
    let settings = get_settings(&app);
    if settings.keyboard_implementation != settings::KeyboardImplementation::HandyKeys {
        return Err(CommandError::new(
            CommandErrorCode::Unsupported,
            "handy-keys is not the active keyboard implementation",
        ));
    }

    // While Secure Input is active the tap receives no KeyDown/KeyUp, so the
    // recorder would silently capture just the modifier and overwrite the
    // binding with it (issue #1578). Refuse instead; the frontend maps this
    // code to a localized explanation, and the noted impact makes the
    // warning banner appear with the full story.
    if crate::secure_input::is_enabled_now() {
        crate::secure_input::note_recorder_blocked(&app);
        return Err(CommandError::new(
            CommandErrorCode::SecureInputActive,
            "Secure input is active",
        ));
    }

    let state = app.try_state::<HandyKeysState>().ok_or_else(|| {
        CommandError::new(CommandErrorCode::Internal, "Input system not initialized")
    })?;

    // Suspend every registered shortcut so a combo that overlaps an existing
    // binding can't fire it (or have its keys swallowed) mid-capture.
    super::suspend_all_shortcuts(&app);

    let result = state
        .start_recording(&app, binding_id)
        .map_err(CommandError::from);
    if result.is_err() {
        super::resume_all_shortcuts(&app);
    }
    result
}

/// Stop key recording mode
#[tauri::command]
#[specta::specta]
pub fn stop_handy_keys_recording(app: AppHandle) -> CommandResult<()> {
    let settings = get_settings(&app);
    if settings.keyboard_implementation != settings::KeyboardImplementation::HandyKeys {
        return Err(CommandError::new(
            CommandErrorCode::Unsupported,
            "handy-keys is not the active keyboard implementation",
        ));
    }

    let state = app.try_state::<HandyKeysState>().ok_or_else(|| {
        CommandError::new(CommandErrorCode::Internal, "Input system not initialized")
    })?;

    // Restore shortcuts from settings regardless of how recording ended.
    // A commit has already registered the new binding via change_binding;
    // re-registering it here fails cleanly and is ignored.
    let result = state.stop_recording().map_err(CommandError::from);
    super::resume_all_shortcuts(&app);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_exponentially_and_caps() {
        let mut backoff = RestartBackoff::new();
        for ms in [100, 200, 400, 800, 1600, 3200, 5000, 5000, 5000] {
            assert_eq!(
                backoff.next_delay(None),
                Some(Duration::from_millis(ms)),
                "delay #{ms}"
            );
        }
    }

    #[test]
    fn backoff_gives_up_at_the_failure_cap() {
        let mut backoff = RestartBackoff::new();
        for _ in 0..RestartBackoff::MAX_FAILURES - 1 {
            assert!(backoff.next_delay(None).is_some());
        }
        assert_eq!(backoff.next_delay(None), None);
        // Once given up, it stays given up.
        assert_eq!(backoff.next_delay(None), None);
        assert_eq!(backoff.next_delay(Some(Duration::from_secs(1))), None);
    }

    #[test]
    fn backoff_resets_after_a_stable_listener_run() {
        let mut backoff = RestartBackoff::new();
        assert_eq!(backoff.next_delay(None), Some(RESTART_BACKOFF_INIT));
        assert_eq!(
            backoff.next_delay(Some(RestartBackoff::STABLE_UPTIME)),
            Some(RESTART_BACKOFF_INIT),
            "a listener that survived the stable window resets the count"
        );
    }

    #[test]
    fn backoff_keeps_growing_below_stable_uptime() {
        // The Windows failure mode this fixes: spawn returns Ok, the hook
        // thread dies right away, and the next death must not restart the
        // delay at the minimum — that was the tight respawn loop.
        let mut backoff = RestartBackoff::new();
        let below_stable = RestartBackoff::STABLE_UPTIME - Duration::from_secs(1);
        assert_eq!(backoff.next_delay(None), Some(RESTART_BACKOFF_INIT));
        assert_eq!(
            backoff.next_delay(Some(below_stable)),
            Some(RESTART_BACKOFF_INIT * 2)
        );
        assert_eq!(
            backoff.next_delay(Some(below_stable)),
            Some(RESTART_BACKOFF_INIT * 4)
        );
    }

    #[test]
    fn validate_rejects_injected_paste_chords() {
        #[cfg(target_os = "macos")]
        let reserved = [
            "command+v",
            "command+shift+v",
            "shift+insert",
            // The injected modifier presses by themselves.
            "command",
            "command+shift",
            "shift",
        ];
        #[cfg(not(target_os = "macos"))]
        let reserved = [
            "ctrl+v",
            "ctrl+shift+v",
            "shift+insert",
            "ctrl",
            "ctrl+shift",
            "shift",
            "ctrl_left+v",
        ];
        for chord in reserved {
            let err = validate_shortcut(chord).expect_err(&format!("{chord} must be rejected"));
            assert!(err.contains("reserved"), "{chord}: {err}");
        }
    }

    #[test]
    fn validate_accepts_non_colliding_shortcuts() {
        for ok in ["ctrl+space", "ctrl+win+space", "f5", "ctrl+alt+v"] {
            assert!(validate_shortcut(ok).is_ok(), "{ok} must be accepted");
        }
    }
}
