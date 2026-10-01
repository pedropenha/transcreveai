use crate::managers::audio::AudioRecordingManager;
use crate::managers::transcription::TranscriptionManager;
use crate::shortcut;
use crate::TranscriptionCoordinator;
use log::info;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Manager};

// Re-export all utility modules for easy access
// pub use crate::audio_feedback::*;
pub use crate::clipboard::*;
pub use crate::overlay::*;
pub use crate::tray::*;

/// Whether the app's debug mode is currently on. `redact_text` only lets
/// diagnostic content through while this is set, so a normal run — debug mode
/// off — never writes transcribed/dictated text to any log target (AC-011-03).
/// Synced at startup and whenever debug mode is toggled (see
/// `shortcut::change_debug_mode_setting`); stays `false` on the headless
/// one-shot path, which also keeps CI logs free of dictation content.
static DEBUG_MODE_ENABLED: AtomicBool = AtomicBool::new(false);

/// Sync the debug-mode flag used by `redact_text`. Called from `lib.rs` setup
/// (including the `--debug` CLI override) and `change_debug_mode_setting`.
pub(crate) fn set_debug_mode_enabled(enabled: bool) {
    DEBUG_MODE_ENABLED.store(enabled, Ordering::Relaxed);
}

/// Preserve diagnostic text while debug mode is on, redact it otherwise.
/// Do not use for secrets such as API keys, which must always be redacted —
/// use `redact_secret_patterns` instead.
pub fn redact_text(text: &str) -> &str {
    if DEBUG_MODE_ENABLED.load(Ordering::Relaxed) {
        text
    } else {
        "[REDACTED]"
    }
}

/// Redact well-known API-key/token patterns from text that came back from an
/// external party (e.g. an LLM provider's error response body) before it is
/// logged or surfaced in error messages (FR-011-03). Covers the patterns the
/// spec calls out — `sk-…`, `gsk_…`, `Bearer …`, `x-api-key: …` — plus a
/// generic `…key…`-style `api_key`/`apikey` assignment as defense in depth.
pub fn redact_secret_patterns(text: &str) -> std::borrow::Cow<'_, str> {
    use regex::Regex;
    use std::sync::LazyLock;

    static PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
        [
            // Bearer / Authorization-style tokens
            r"(?i)\bbearer\s+[A-Za-z0-9._~+/=-]+",
            // OpenAI-style and Groq-style key prefixes
            r"\bsk-[A-Za-z0-9_-]{4,}",
            r"\bgsk_[A-Za-z0-9_-]{4,}",
            // Header/json-ish assignments: x-api-key: …, api_key = …,
            // "authorization": "…" — the optional quote covers JSON bodies.
            r#"(?i)(x-api-key|api[_-]?key|authorization)["']?\s*[:=]\s*["']?[A-Za-z0-9._~+/=-]{4,}["']?"#,
        ]
        .iter()
        .map(|p| Regex::new(p).expect("static secret redaction pattern must compile"))
        .collect()
    });

    // Only allocate when something actually matched — the common case is clean.
    let mut redacted: std::borrow::Cow<'_, str> = text.into();
    for pattern in PATTERNS.iter() {
        if pattern.is_match(&redacted) {
            redacted = pattern
                .replace_all(&redacted, "[REDACTED]")
                .into_owned()
                .into();
        }
    }
    redacted
}

#[cfg(any(test, all(target_os = "windows", target_arch = "x86_64")))]
const IMAGE_FILE_MACHINE_ARM64: u16 = 0xaa64;

#[cfg(any(test, all(target_os = "windows", target_arch = "x86_64")))]
fn native_machine_is_arm64(native_machine: Option<u16>) -> bool {
    native_machine == Some(IMAGE_FILE_MACHINE_ARM64)
}

/// Whether this is the x64 Windows build running under emulation on Windows ARM64.
///
/// Only that exact process/host pairing disables the transcribe.cpp GPU path.
/// Detection is deliberately fail-open: a native x64 host, an older Windows
/// version without `IsWow64Process2`, or any API error leaves existing behavior
/// unchanged.
pub fn is_windows_x64_emulated_on_arm64() -> bool {
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        use std::sync::OnceLock;

        static DETECTED: OnceLock<bool> = OnceLock::new();
        *DETECTED.get_or_init(|| native_machine_is_arm64(native_windows_machine()))
    }

    #[cfg(not(all(target_os = "windows", target_arch = "x86_64")))]
    {
        false
    }
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
fn native_windows_machine() -> Option<u16> {
    use windows::core::{s, w, BOOL};
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    use windows::Win32::System::Threading::GetCurrentProcess;

    type IsWow64Process2 = unsafe extern "system" fn(HANDLE, *mut u16, *mut u16) -> BOOL;

    // Resolve IsWow64Process2 dynamically so merely starting Handy never raises
    // the minimum Windows version. Windows-on-ARM versions provide this API,
    // while a missing symbol or failed query safely preserves the x64 behavior.
    unsafe {
        let kernel32 = GetModuleHandleW(w!("kernel32.dll")).ok()?;
        let address = GetProcAddress(kernel32, s!("IsWow64Process2"))?;
        // SAFETY: GetProcAddress returned the documented IsWow64Process2 symbol;
        // function pointers have the same representation on supported Windows.
        let is_wow64_process2: IsWow64Process2 = std::mem::transmute(address);
        let mut process_machine = 0u16;
        let mut native_machine = 0u16;
        is_wow64_process2(
            GetCurrentProcess(),
            &mut process_machine,
            &mut native_machine,
        )
        .as_bool()
        .then_some(native_machine)
    }
}

/// Disable WebView2 browser accelerators (F5, F6, Ctrl+F, F12, ...) on an app
/// window. A settings/notes window has no use for them, and pressing F6 while
/// recording a shortcut was reported to turn the whole window white
/// (cjpais/Handy#1940), likely by triggering WebView2 focus cycling. DevTools
/// stays enabled; only the F12 accelerator is lost. Shared by the Hub and the
/// meeting window (T-066) so the WebView2 FFI lives in exactly one place.
#[cfg(target_os = "windows")]
pub fn disable_webview2_accelerators(window: &tauri::WebviewWindow) {
    let _ = window.with_webview(|webview| unsafe {
        use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Settings3;
        use windows::core::Interface;

        let result = webview
            .controller()
            .CoreWebView2()
            .and_then(|core| core.Settings())
            .and_then(|settings| settings.cast::<ICoreWebView2Settings3>())
            .and_then(|settings| settings.SetAreBrowserAcceleratorKeysEnabled(false));

        if let Err(error) = result {
            log::warn!("Failed to disable WebView2 browser accelerators: {error}");
        }
    });
}

/// Centralized cancellation function that can be called from anywhere in the app.
/// Handles cancelling both recording and transcription operations and updates UI state.
pub fn cancel_current_operation(app: &AppHandle) {
    info!("Initiating operation cancellation...");

    // Unregister the cancel shortcut asynchronously
    shortcut::unregister_cancel_shortcut(app);

    // Cancel any ongoing recording
    let audio_manager = app.state::<Arc<AudioRecordingManager>>();
    let recording_was_active = audio_manager.is_recording();
    audio_manager.cancel_recording();

    // Abandon any live streaming transcription
    let tm = app.state::<Arc<TranscriptionManager>>();
    tm.cancel_stream();

    // Update tray icon and hide overlay
    set_tray_state(app, crate::tray::TrayIconState::Idle);
    hide_recording_overlay(app);

    // Unload model if immediate unload is enabled
    tm.maybe_unload_immediately("cancellation");

    // Notify coordinator so it can keep lifecycle state coherent.
    if let Some(coordinator) = app.try_state::<TranscriptionCoordinator>() {
        coordinator.notify_cancel(recording_was_active);
    }

    info!("Operation cancellation completed - returned to idle state");
}

/// Check if using the Wayland display server protocol
#[cfg(target_os = "linux")]
pub fn is_wayland() -> bool {
    std::env::var("WAYLAND_DISPLAY").is_ok()
        || std::env::var("XDG_SESSION_TYPE")
            .map(|v| v.to_lowercase() == "wayland")
            .unwrap_or(false)
}

/// Check if running on KDE Plasma desktop environment
#[cfg(target_os = "linux")]
pub fn is_kde_plasma() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP")
        .map(|v| v.to_uppercase().contains("KDE"))
        .unwrap_or(false)
        || std::env::var("KDE_SESSION_VERSION").is_ok()
}

/// Check if running on KDE Plasma with Wayland
#[cfg(target_os = "linux")]
pub fn is_kde_wayland() -> bool {
    is_wayland() && is_kde_plasma()
}

/// Check if running on GNOME desktop environment
#[cfg(target_os = "linux")]
pub fn is_gnome() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP")
        .map(|v| v.to_uppercase().contains("GNOME"))
        .unwrap_or(false)
}

/// Check if running on GNOME with Wayland
#[cfg(target_os = "linux")]
pub fn is_gnome_wayland() -> bool {
    is_wayland() && is_gnome()
}

/// Returns true when the environment variable is set to a truthy value
/// (e.g. "1", "true", "yes", "on").
/// "0", "false", "no", "off" and empty string are treated as falsy (case-insensitive).
/// Returns false when the variable is not set.
pub fn env_flag_enabled(name: &str) -> bool {
    match std::env::var(name) {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "" | "0" | "false" | "no" | "off"
        ),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arm64_native_machine_is_the_only_match() {
        assert!(native_machine_is_arm64(Some(IMAGE_FILE_MACHINE_ARM64)));
        assert!(!native_machine_is_arm64(Some(0x8664))); // AMD64
        assert!(!native_machine_is_arm64(Some(0x014c))); // I386
        assert!(!native_machine_is_arm64(None)); // API unavailable or failed
    }

    #[test]
    fn env_flag_enabled_true_for_truthy_values() {
        for value in ["1", "true", "TRUE", "yes", "on", " 1 "] {
            std::env::set_var("TRANSCREVE_TEST_FLAG_TRUTHY", value);
            assert!(env_flag_enabled("TRANSCREVE_TEST_FLAG_TRUTHY"), "{value:?}");
        }
        std::env::remove_var("TRANSCREVE_TEST_FLAG_TRUTHY");
    }

    #[test]
    fn env_flag_enabled_false_for_falsy_or_unset() {
        assert!(!env_flag_enabled("TRANSCREVE_TEST_FLAG_UNSET"));

        for value in ["0", "false", "FALSE", "no", "off", ""] {
            std::env::set_var("TRANSCREVE_TEST_FLAG_FALSY", value);
            assert!(!env_flag_enabled("TRANSCREVE_TEST_FLAG_FALSY"), "{value:?}");
        }
        std::env::remove_var("TRANSCREVE_TEST_FLAG_FALSY");
    }

    /// AC-011-03: with debug mode off, transcribed text never reaches the logs —
    /// only "[REDACTED]" does. With debug mode on it passes through so the live
    /// log viewer remains useful for diagnosing dictation problems.
    #[test]
    fn redact_text_follows_runtime_debug_mode() {
        let prev = DEBUG_MODE_ENABLED.load(Ordering::Relaxed);

        set_debug_mode_enabled(false);
        assert_eq!(redact_text("o usuário ditou isto"), "[REDACTED]");

        set_debug_mode_enabled(true);
        assert_eq!(redact_text("o usuário ditou isto"), "o usuário ditou isto");

        set_debug_mode_enabled(prev);
    }

    /// FR-011-03: API keys and auth tokens must never survive into log lines,
    /// even inside third-party response bodies.
    #[test]
    fn redact_secret_patterns_scrubs_known_key_formats() {
        for input in [
            "invalid key sk-abc123DEF456",
            "Unauthorized: gsk_XyZ1234567890",
            "bad header Bearer eyJhbGciOiJIUzI1NiJ9.payload.sig",
            "x-api-key: AIzaSyD-secret",
            r#"{"error":{"api_key":"sk-live-9999"}}"#,
        ] {
            let out = redact_secret_patterns(input);
            assert!(out.contains("[REDACTED]"), "input was not scrubbed: {out}");
            assert!(!out.contains("abc123DEF456"));
            assert!(!out.contains("XyZ1234567890"));
            assert!(!out.contains("eyJhbGciOiJIUzI1NiJ9"));
            assert!(!out.contains("AIzaSyD-secret"));
            assert!(!out.contains("sk-live-9999"));
        }
    }

    #[test]
    fn redact_secret_patterns_leaves_clean_text_untouched() {
        let clean = "API request failed with status 429: rate limit exceeded";
        assert_eq!(redact_secret_patterns(clean), clean);
    }
}
