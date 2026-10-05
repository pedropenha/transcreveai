//! Text insertion dispatcher (F005, T-031).
//!
//! `settings.insertion_method` is the v1 contract (ADR-0002: `auto` is the
//! default). This module resolves it into a concrete [`InsertionPlan`]:
//!
//! - `auto` pastes (Ctrl+V by default) and switches to direct typing for the
//!   known remote/legacy targets from the spec table (`mstsc.exe`,
//!   `wfica32.exe`, `vmconnect.exe`, `CDViewer.exe`). On Linux `auto` resolves
//!   to typing — the platform's historical default.
//! - A foreground target running at a higher integrity level (UIPI, an
//!   elevated app) cannot receive injected input (FR-005-07, AC-005-05);
//!   likewise no valid focus window (desktop, lock screen, UAC prompt)
//!   (FR-005-08). Both fall back to `clipboard_only` and emit
//!   [`CLIPBOARD_ONLY_WARNING_EVENT`].
//! - `clipboard_only_on_window_change` (FR-005-09, off by default) degrades
//!   to `clipboard_only` when the focused window changed between the hotkey
//!   release and the delivery.
//!
//! Reconciliation with the legacy `paste_method` (the lane's known debt):
//! `insertion_method` decides *the strategy*; `paste_method` still selects
//! *which chord* a `Paste` plan sends (so a stored `ctrl_shift_v` keeps
//! producing Ctrl+Shift+V) and remains the escape hatch for
//! `external_script`. The migration in `apply_settings_migrations` maps the
//! legacy value onto `insertion_method` when the new key is absent.
//!
//! FR-005-10: insertions are serialized — the coordinator drains one session
//! at a time and `clipboard::paste` runs on the main thread; a second session
//! can only start pasting after the previous `run_on_main_thread` closure
//! returned. (The reliable-paste transaction additionally settles any pending
//! transaction before publishing a new one.)

use crate::settings::{InsertionMethod, NewlineMode, PasteMethod};
use enigo::{Direction, Enigo, Key, Keyboard};
use log::warn;
use serde::Serialize;
use std::time::Duration;

/// Event emitted whenever an insertion degrades to `clipboard_only`
/// (FR-005-07/08/09): the text is on the clipboard and the user must paste it
/// themselves. The toast UI is a follow-up; the payload is stable so the
/// frontend can already attach a listener.
pub const CLIPBOARD_ONLY_WARNING_EVENT: &str = "insertion://clipboard-only-warning";

/// Per-character typing floor for remote/legacy targets under `auto`
/// (FR-005-05: "5 ms/char para RDP").
pub(crate) const REMOTE_TYPE_CHAR_DELAY_MS: u64 = 5;

/// Above this length direct typing is impractical (spec edge case): the
/// explicit `type` method prefers paste instead. `auto`-resolved typing is
/// exempt — its targets are remote sessions where paste cannot reach.
pub(crate) const TYPE_LONG_TEXT_LEN: usize = 5_000;

/// Exes where `auto` types instead of pasting (spec methods table).
const TYPE_PREFERRED_EXES: &[&str] = &[
    "mstsc.exe",     // Remote Desktop
    "wfica32.exe",   // Citrix
    "vmconnect.exe", // Hyper-V VM console
    "cdviewer.exe",  // Citrix Viewer
];

/// Why an insertion delivered `clipboard_only` — serialized as `reason` on
/// [`CLIPBOARD_ONLY_WARNING_EVENT`] and recorded in the dictation row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardOnlyReason {
    /// The user configured `clipboard_only` themselves.
    Requested,
    /// FR-005-07: the focused window runs at a higher integrity level (UIPI).
    ElevatedTarget,
    /// FR-005-08: no valid window in focus (desktop, lock screen, UAC).
    NoForegroundTarget,
    /// FR-005-09: focus moved since session start with
    /// `clipboard_only_on_window_change` on.
    WindowChanged,
}

impl ClipboardOnlyReason {
    /// Machine-readable id, identical to the serde representation.
    pub fn as_str(&self) -> &'static str {
        match self {
            ClipboardOnlyReason::Requested => "requested",
            ClipboardOnlyReason::ElevatedTarget => "elevated_target",
            ClipboardOnlyReason::NoForegroundTarget => "no_foreground_target",
            ClipboardOnlyReason::WindowChanged => "window_changed",
        }
    }
}

/// Payload of [`CLIPBOARD_ONLY_WARNING_EVENT`].
#[derive(Debug, Clone, Serialize)]
pub struct ClipboardOnlyWarning {
    pub reason: ClipboardOnlyReason,
    /// Foreground process exe (e.g. "powershell.exe") when known.
    pub exe_name: Option<String>,
}

/// What one foreground probe learned about the insertion target. On platforms
/// without probing (macOS/Linux today) [`ForegroundTarget::unprobed`]
/// disables every target-based gate.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForegroundTarget {
    /// The platform actually probed the foreground window.
    pub probed: bool,
    /// A valid, usable window is focused (false for desktop/lockscreen/UAC).
    pub has_window: bool,
    /// Opaque window id (HWND on Windows), for the FR-005-09 change check.
    pub window_id: Option<usize>,
    /// Owning process exe name (e.g. "slack.exe"), original case.
    pub exe_name: Option<String>,
    /// The target runs at a higher integrity level than we do.
    pub elevated: bool,
}

impl ForegroundTarget {
    /// No probing on this platform — every gate passes. Only exists where a
    /// probe doesn't run (non-Windows) or for tests that build a fake target.
    #[cfg(any(not(target_os = "windows"), test))]
    pub fn unprobed() -> Self {
        Self::default()
    }
}

/// The concrete delivery the dispatcher picked for one insertion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertionPlan {
    /// Clipboard paste; the carried `PasteMethod` is the chord (CtrlV,
    /// CtrlShiftV or ShiftInsert — never another variant).
    Paste(PasteMethod),
    /// Direct typing (enigo); `newline_mode` decides how `\n` is sent.
    Type {
        newline_mode: NewlineMode,
        char_delay_ms: u64,
    },
    /// Only copy + warn (see [`ClipboardOnlyReason`]).
    ClipboardOnly(ClipboardOnlyReason),
    /// Legacy escape hatch: run the configured external script.
    ExternalScript,
}

impl InsertionPlan {
    /// Settings string recorded in history (`insert_method`, FR-005-11).
    pub fn method_name(&self) -> &'static str {
        match self {
            InsertionPlan::Paste(PasteMethod::CtrlV) => "paste",
            InsertionPlan::Paste(PasteMethod::CtrlShiftV) => "paste_ctrl_shift_v",
            InsertionPlan::Paste(PasteMethod::ShiftInsert) => "paste_shift_insert",
            InsertionPlan::Paste(_) => "paste",
            InsertionPlan::Type { .. } => "type",
            InsertionPlan::ClipboardOnly(_) => "clipboard_only",
            InsertionPlan::ExternalScript => "external_script",
        }
    }

    /// Whether the pipeline may send the auto-submit key after this plan
    /// (FR-005-06). `clipboard_only` does not touch the target, so submitting
    /// would press Enter into it for nothing.
    pub fn sends_auto_submit(&self) -> bool {
        !matches!(self, InsertionPlan::ClipboardOnly(_))
    }
}

/// History status of a finished insertion (dictations `status`,
/// FR-005-11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertionStatus {
    Inserted,
    Copied,
    Failed,
}

impl InsertionStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            InsertionStatus::Inserted => "inserted",
            InsertionStatus::Copied => "copied",
            InsertionStatus::Failed => "failed",
        }
    }
}

/// Outcome of one insertion, recorded on the session result event and in the
/// dictation history row (FR-005-11).
#[derive(Debug, Clone)]
pub struct InsertionReport {
    pub status: InsertionStatus,
    /// Plan actually executed (after `auto` resolution and UIPI fallback).
    pub plan: InsertionPlan,
    /// `insertion_method` as configured (before resolution).
    pub requested: InsertionMethod,
    /// Set when the plan degraded to `clipboard_only`.
    pub clipboard_only_reason: Option<ClipboardOnlyReason>,
    /// Wall time of the insert dispatch (`insert_ms` in `latency_json`).
    pub elapsed: Duration,
    /// Why delivery failed, when `status` is [`InsertionStatus::Failed`]
    /// (also notes a post-insert clipboard-copy hiccup).
    pub error: Option<String>,
}

/// Inputs for [`resolve_plan`], bundled so the decision stays a pure
/// function (unit tests build it directly; `clipboard::paste` fills it from
/// settings + a live probe).
#[derive(Debug)]
pub struct InsertionContext {
    pub requested: InsertionMethod,
    pub paste_method: PasteMethod,
    pub newline_mode: NewlineMode,
    pub type_char_delay_ms: u64,
    pub clipboard_only_on_window_change: bool,
    /// Chars about to be inserted (for the long-text `type` guard).
    pub text_len: usize,
    pub target: ForegroundTarget,
    /// Foreground window captured at session start (FR-005-09); `None` when
    /// there is no live session (e.g. paste-last) or the platform doesn't
    /// probe.
    pub session_window: Option<usize>,
}

/// Which chord a `Paste` plan sends: the legacy `paste_method` when it names
/// a chord, else Ctrl+V.
fn paste_chord(paste_method: PasteMethod) -> PasteMethod {
    match paste_method {
        PasteMethod::CtrlV | PasteMethod::CtrlShiftV | PasteMethod::ShiftInsert => paste_method,
        _ => PasteMethod::CtrlV,
    }
}

/// `auto` targets that cannot receive a clipboard paste (remote/legacy
/// sessions) get direct typing instead. Case-insensitive exe match.
fn exe_prefers_typing(exe_name: Option<&str>) -> bool {
    exe_name.is_some_and(|exe| {
        let exe = exe.to_ascii_lowercase();
        TYPE_PREFERRED_EXES.contains(&exe.as_str())
    })
}

/// Resolve the effective insertion plan (FR-005-05..09). Pure — every OS
/// fact arrives via `ctx.target`.
///
/// Order matters: the external-script escape hatch wins unconditionally; then
/// target gates (no window / elevated / window changed) degrade any injected
/// delivery to `clipboard_only`; only then does the configured method apply.
pub(crate) fn resolve_plan(ctx: &InsertionContext) -> InsertionPlan {
    // Legacy escape hatch: an explicit external script keeps running whatever
    // the new method says (the script is the user's own delivery mechanism).
    if ctx.paste_method == PasteMethod::ExternalScript {
        return InsertionPlan::ExternalScript;
    }

    if ctx.target.probed {
        if !ctx.target.has_window {
            return InsertionPlan::ClipboardOnly(ClipboardOnlyReason::NoForegroundTarget);
        }
        // FR-005-07 / AC-005-05: injecting into a higher-integrity process is
        // silently dropped by UIPI — never try, copy + warn instead.
        if ctx.target.elevated {
            return InsertionPlan::ClipboardOnly(ClipboardOnlyReason::ElevatedTarget);
        }
        if ctx.clipboard_only_on_window_change
            && ctx
                .session_window
                .is_some_and(|w| ctx.target.window_id != Some(w))
        {
            return InsertionPlan::ClipboardOnly(ClipboardOnlyReason::WindowChanged);
        }
    }

    let type_plan = |char_delay_ms: u64| InsertionPlan::Type {
        newline_mode: ctx.newline_mode,
        char_delay_ms,
    };

    match ctx.requested {
        InsertionMethod::ClipboardOnly => {
            InsertionPlan::ClipboardOnly(ClipboardOnlyReason::Requested)
        }
        InsertionMethod::Paste => InsertionPlan::Paste(paste_chord(ctx.paste_method)),
        InsertionMethod::PasteShiftInsert => InsertionPlan::Paste(PasteMethod::ShiftInsert),
        InsertionMethod::Type => {
            if ctx.text_len > TYPE_LONG_TEXT_LEN {
                // Spec edge case: huge `type` payloads are slow — warn and
                // prefer paste when the user asked for typing explicitly.
                warn!(
                    "type insertion of {} chars exceeds {} — falling back to paste",
                    ctx.text_len, TYPE_LONG_TEXT_LEN
                );
                InsertionPlan::Paste(paste_chord(ctx.paste_method))
            } else {
                type_plan(ctx.type_char_delay_ms)
            }
        }
        InsertionMethod::Auto => {
            if exe_prefers_typing(ctx.target.exe_name.as_deref()) {
                if ctx.text_len > TYPE_LONG_TEXT_LEN {
                    // Same edge case, but paste cannot reach a remote session
                    // without a shared clipboard — keep typing, slowly.
                    warn!(
                        "type insertion of {} chars into '{}' will be slow",
                        ctx.text_len,
                        ctx.target.exe_name.as_deref().unwrap_or("?")
                    );
                }
                type_plan(ctx.type_char_delay_ms.max(REMOTE_TYPE_CHAR_DELAY_MS))
            } else {
                auto_fallback(ctx)
            }
        }
    }
}

/// `auto` on an ordinary target: paste on Windows/macOS; on Linux the
/// historic default is direct typing (clipboard paste is unreliable on
/// Wayland), matching what `paste_method = "direct"` used to do.
fn auto_fallback(ctx: &InsertionContext) -> InsertionPlan {
    #[cfg(target_os = "linux")]
    {
        InsertionPlan::Type {
            newline_mode: ctx.newline_mode,
            char_delay_ms: ctx.type_char_delay_ms,
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        InsertionPlan::Paste(paste_chord(ctx.paste_method))
    }
}

// ---------------------------------------------------------------------------
// Direct typing (FR-005-05)
// ---------------------------------------------------------------------------

/// One unit of typing work. Text runs go through `enigo.text()`; newlines are
/// explicit keys so `newline_mode` — not the input library — decides what a
/// `\n` does (AC-005-07).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TypeSegment {
    Text(String),
    /// `\n` sent as Enter (`newline_mode = "raw"`).
    Enter,
    /// `\n` sent as Shift+Enter (`newline_mode = "shift_enter"`).
    ShiftEnter,
}

/// Split `text` into typing segments. `\r\n`, lone `\r` and `\n` all count as
/// one newline; empty runs produce no `Text` segment.
pub(crate) fn type_segments(text: &str, newline_mode: NewlineMode) -> Vec<TypeSegment> {
    let newline = match newline_mode {
        NewlineMode::Raw => TypeSegment::Enter,
        NewlineMode::ShiftEnter => TypeSegment::ShiftEnter,
    };
    let mut segments = Vec::new();
    let mut run = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let is_newline = match c {
            '\n' => true,
            '\r' => {
                // Consume the `\n` of a `\r\n` pair so it counts once.
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                true
            }
            _ => false,
        };
        if is_newline {
            if !run.is_empty() {
                segments.push(TypeSegment::Text(std::mem::take(&mut run)));
            }
            segments.push(newline.clone());
        } else {
            run.push(c);
        }
    }
    if !run.is_empty() {
        segments.push(TypeSegment::Text(run));
    }
    segments
}

/// Type `text` char-by-char through enigo (FR-005-05). `char_delay_ms` paces
/// every emitted unit (segment or keypress) for remote targets.
pub(crate) fn type_text_direct(
    enigo: &mut Enigo,
    text: &str,
    newline_mode: NewlineMode,
    char_delay_ms: u64,
) -> Result<(), String> {
    type_text_direct_guarded(enigo, text, newline_mode, char_delay_ms, &|| Ok(()), false)
}

/// Session guard is checked after modifier waits and before every typed unit.
pub(crate) fn type_text_direct_guarded(
    enigo: &mut Enigo,
    text: &str,
    newline_mode: NewlineMode,
    char_delay_ms: u64,
    can_deliver: &dyn Fn() -> Result<(), String>,
    per_character: bool,
) -> Result<(), String> {
    // FR-005-01 also guards direct typing: a held Ctrl would turn typed
    // characters into shortcuts (e.g. "x" becoming Ctrl+X). The wait must
    // precede the guard — it depends on seeing the user's own key releases.
    crate::input::await_shortcut_modifier_release();
    let _guard = crate::input::InjectionGuard::begin();
    let pause = || {
        if char_delay_ms > 0 {
            std::thread::sleep(Duration::from_millis(char_delay_ms));
        }
    };
    for segment in type_segments(text, newline_mode) {
        can_deliver()?;
        match segment {
            TypeSegment::Text(run) => {
                if char_delay_ms > 0 || per_character {
                    for c in run.chars() {
                        can_deliver()?;
                        enigo
                            .text(&c.to_string())
                            .map_err(|e| format!("Failed to type character: {e}"))?;
                        pause();
                    }
                } else {
                    enigo
                        .text(&run)
                        .map_err(|e| format!("Failed to type text: {e}"))?;
                }
            }
            TypeSegment::Enter => {
                enigo
                    .key(Key::Return, Direction::Click)
                    .map_err(|e| format!("Failed to send Enter: {e}"))?;
                pause();
            }
            TypeSegment::ShiftEnter => {
                enigo
                    .key(Key::Shift, Direction::Press)
                    .map_err(|e| format!("Failed to press Shift: {e}"))?;
                enigo
                    .key(Key::Return, Direction::Click)
                    .map_err(|e| format!("Failed to send Return: {e}"))?;
                enigo
                    .key(Key::Shift, Direction::Release)
                    .map_err(|e| format!("Failed to release Shift: {e}"))?;
                pause();
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Session window tracking (FR-005-09)
// ---------------------------------------------------------------------------

/// Foreground window captured when the session's recording stopped (the
/// moment the user finished dictating in the target window), consumed by the
/// insertion at the end of the pipeline. `AtomicIsize` holds the HWND bits;
/// 0 = none. Sessions are serialized by the coordinator, so a single slot
/// suffices — each session overwrites it at its own stop.
#[cfg(target_os = "windows")]
static SESSION_WINDOW: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

/// Called when a dictation session stops recording: remembers which window
/// was focused then, so insertion can detect a focus change before the text
/// arrives (FR-005-09). A no-op off Windows (no probe there either).
pub(crate) fn note_session_window() {
    #[cfg(target_os = "windows")]
    {
        use std::sync::atomic::Ordering;
        let hwnd = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
        SESSION_WINDOW.store(hwnd.0 as isize, Ordering::SeqCst);
    }
}

/// Take (consume) the session-start window token. Called once by the session
/// insertion path; `paste_last` passes `None` and never consumes it.
pub(crate) fn take_session_window() -> Option<usize> {
    #[cfg(target_os = "windows")]
    {
        use std::sync::atomic::Ordering;
        match SESSION_WINDOW.swap(0, Ordering::SeqCst) {
            0 => None,
            raw => Some(raw as usize),
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

// ---------------------------------------------------------------------------
// Foreground target probe (Windows)
// ---------------------------------------------------------------------------

/// Snapshot the insertion target right before dispatching.
pub(crate) fn probe_foreground_target() -> ForegroundTarget {
    #[cfg(target_os = "windows")]
    {
        windows_probe::probe()
    }
    #[cfg(not(target_os = "windows"))]
    {
        ForegroundTarget::unprobed()
    }
}

#[cfg(target_os = "windows")]
mod windows_probe {
    use super::ForegroundTarget;
    use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND};
    use windows::Win32::Security::{
        GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TokenIntegrityLevel,
        TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, OpenProcessToken, QueryFullProcessImageNameW,
        PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetForegroundWindow, GetWindowThreadProcessId,
    };

    /// Medium integrity — the level a normal (non-elevated) process runs at.
    /// Used as the fallback baseline when our own token cannot be queried.
    const SECURITY_MANDATORY_MEDIUM_RID: u32 = 0x2000;

    /// Window classes that are the desktop rather than a real target
    /// (FR-005-08 "área de trabalho"): Program Manager and the shell's
    /// WorkerW wallpaper host.
    const SHELL_DESKTOP_CLASSES: &[&str] = &["Progman", "WorkerW"];

    struct HandleGuard(HANDLE);
    impl Drop for HandleGuard {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    /// Integrity-level RID of the process behind `handle` (0x1000 low,
    /// 0x2000 medium, 0x3000 high, 0x4000 system), or `None` when the token
    /// cannot be read.
    fn integrity_level(handle: HANDLE) -> Option<u32> {
        unsafe {
            let mut token = HANDLE::default();
            OpenProcessToken(handle, TOKEN_QUERY, &mut token).ok()?;
            let _token_guard = HandleGuard(token);

            let mut len = 0u32;
            let _ = GetTokenInformation(token, TokenIntegrityLevel, None, 0, &mut len);
            if len == 0 {
                return None;
            }
            let mut buf = vec![0u8; len as usize];
            GetTokenInformation(
                token,
                TokenIntegrityLevel,
                Some(buf.as_mut_ptr() as *mut _),
                len,
                &mut len,
            )
            .ok()?;

            let label = &*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL);
            let sid = label.Label.Sid;
            if sid.0.is_null() {
                return None;
            }
            let count = u32::from(*GetSidSubAuthorityCount(sid));
            if count == 0 {
                return None;
            }
            Some(*GetSidSubAuthority(sid, count - 1))
        }
    }

    /// Whether the process `pid` runs at a higher integrity level than this
    /// process (UIPI would silently drop our injected input). Fail-safe:
    /// `OpenProcess`/token failures count as elevated — the common cause is
    /// exactly the access check we're probing for.
    fn process_is_elevated_target(pid: u32) -> bool {
        unsafe {
            let ours =
                integrity_level(GetCurrentProcess()).unwrap_or(SECURITY_MANDATORY_MEDIUM_RID);
            let target = match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                Ok(handle) => {
                    let level = {
                        let _guard = HandleGuard(handle);
                        integrity_level(handle)
                    };
                    match level {
                        Some(level) => level,
                        None => return true,
                    }
                }
                Err(_) => return true,
            };
            target > ours
        }
    }

    /// Full image path of `pid`, `None` when the process can't be opened.
    fn process_image_name(pid: u32) -> Option<String> {
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let _guard = HandleGuard(handle);
            let mut buf = vec![0u16; 1024];
            let mut len = buf.len() as u32;
            QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_WIN32,
                windows::core::PWSTR(buf.as_mut_ptr()),
                &mut len,
            )
            .ok()?;
            Some(String::from_utf16_lossy(&buf[..len as usize]))
        }
    }

    /// The window is usable only when it isn't the desktop itself (Progman /
    /// WorkerW — FR-005-08). Class read failure keeps the window usable:
    /// blocking on an unreadable class would disable dictation for a whole
    /// class of perfectly normal apps.
    fn is_desktop_window(hwnd: HWND) -> bool {
        let mut buf = [0u16; 64];
        let len = unsafe { GetClassNameW(hwnd, &mut buf) };
        if len <= 0 {
            return false;
        }
        let class = String::from_utf16_lossy(&buf[..len as usize]);
        SHELL_DESKTOP_CLASSES.contains(&class.as_str())
    }

    pub fn probe() -> ForegroundTarget {
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.0.is_null() {
            // Lock screen, secure desktop (UAC), or no focus at all.
            return ForegroundTarget {
                probed: true,
                has_window: false,
                ..Default::default()
            };
        }
        if is_desktop_window(hwnd) {
            return ForegroundTarget {
                probed: true,
                has_window: false,
                window_id: Some(hwnd.0 as usize),
                ..Default::default()
            };
        }
        let mut pid: u32 = 0;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if pid == 0 {
            return ForegroundTarget {
                probed: true,
                has_window: false,
                window_id: Some(hwnd.0 as usize),
                ..Default::default()
            };
        }
        let exe_name = process_image_name(pid).map(|path| {
            path.rsplit(['\\', '/'])
                .next()
                .unwrap_or(path.as_str())
                .to_string()
        });
        ForegroundTarget {
            probed: true,
            has_window: true,
            window_id: Some(hwnd.0 as usize),
            exe_name,
            elevated: process_is_elevated_target(pid),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> InsertionContext {
        InsertionContext {
            requested: InsertionMethod::Auto,
            paste_method: PasteMethod::CtrlV,
            newline_mode: NewlineMode::Raw,
            type_char_delay_ms: 0,
            clipboard_only_on_window_change: false,
            text_len: 10,
            target: ForegroundTarget {
                probed: true,
                has_window: true,
                window_id: Some(0x1000),
                exe_name: Some("notepad.exe".into()),
                elevated: false,
            },
            session_window: Some(0x1000),
        }
    }

    // --- AC-005-05 / FR-005-07: elevated target → clipboard_only -------------

    #[test]
    fn elevated_target_falls_back_to_clipboard_only_for_every_method() {
        let mut c = ctx();
        c.target.elevated = true;
        for requested in [
            InsertionMethod::Auto,
            InsertionMethod::Paste,
            InsertionMethod::PasteShiftInsert,
            InsertionMethod::Type,
            InsertionMethod::ClipboardOnly,
        ] {
            c.requested = requested;
            let plan = resolve_plan(&c);
            assert!(
                matches!(plan, InsertionPlan::ClipboardOnly(_)),
                "{requested:?} must not inject into an elevated window"
            );
        }
        // The explicit methods report the UIPI reason; clipboard_only keeps
        // the honest "elevated" reason (it's still why nothing was injected).
        c.requested = InsertionMethod::Type;
        assert_eq!(
            resolve_plan(&c),
            InsertionPlan::ClipboardOnly(ClipboardOnlyReason::ElevatedTarget)
        );
    }

    // --- FR-005-08: no usable window → clipboard_only -------------------------

    #[test]
    fn no_foreground_window_falls_back_to_clipboard_only() {
        let mut c = ctx();
        c.target.has_window = false;
        c.target.window_id = None;
        assert_eq!(
            resolve_plan(&c),
            InsertionPlan::ClipboardOnly(ClipboardOnlyReason::NoForegroundTarget)
        );
    }

    #[test]
    fn unprobed_platform_skips_target_gates() {
        let mut c = ctx();
        c.target = ForegroundTarget::unprobed();
        c.requested = InsertionMethod::Paste;
        assert_eq!(resolve_plan(&c), InsertionPlan::Paste(PasteMethod::CtrlV));
    }

    // --- FR-005-09: window changed -------------------------------------------

    #[test]
    fn window_change_copies_only_when_option_enabled() {
        let mut c = ctx();
        c.target.window_id = Some(0x2222); // focus moved since session start
        c.clipboard_only_on_window_change = true;
        assert_eq!(
            resolve_plan(&c),
            InsertionPlan::ClipboardOnly(ClipboardOnlyReason::WindowChanged)
        );

        // Off by default: insert into the *current* window anyway.
        c.clipboard_only_on_window_change = false;
        assert_eq!(resolve_plan(&c), InsertionPlan::Paste(PasteMethod::CtrlV));

        // Same window as at session start → insert normally.
        c.clipboard_only_on_window_change = true;
        c.target.window_id = Some(0x1000);
        assert_eq!(resolve_plan(&c), InsertionPlan::Paste(PasteMethod::CtrlV));
    }

    // --- auto heuristic -------------------------------------------------------

    #[test]
    #[cfg(not(target_os = "linux"))]
    fn auto_defaults_to_paste_for_ordinary_apps() {
        let c = ctx();
        assert_eq!(resolve_plan(&c), InsertionPlan::Paste(PasteMethod::CtrlV));
    }

    #[test]
    fn auto_types_into_known_remote_exes() {
        for exe in ["mstsc.exe", "WFICA32.EXE", "vmconnect.exe", "CDViewer.exe"] {
            let mut c = ctx();
            c.target.exe_name = Some(exe.into());
            let plan = resolve_plan(&c);
            assert!(
                matches!(
                    plan,
                    InsertionPlan::Type {
                        char_delay_ms: REMOTE_TYPE_CHAR_DELAY_MS,
                        ..
                    }
                ),
                "{exe} should type at the remote floor rate, got {plan:?}"
            );
        }
    }

    #[test]
    fn auto_type_keeps_a_larger_configured_delay() {
        let mut c = ctx();
        c.target.exe_name = Some("mstsc.exe".into());
        c.type_char_delay_ms = 20;
        assert_eq!(
            resolve_plan(&c),
            InsertionPlan::Type {
                newline_mode: NewlineMode::Raw,
                char_delay_ms: 20,
            }
        );
    }

    // --- explicit methods -----------------------------------------------------

    #[test]
    fn explicit_methods_resolve_directly() {
        let mut c = ctx();
        c.requested = InsertionMethod::PasteShiftInsert;
        assert_eq!(
            resolve_plan(&c),
            InsertionPlan::Paste(PasteMethod::ShiftInsert)
        );
        c.requested = InsertionMethod::Type;
        assert_eq!(
            resolve_plan(&c),
            InsertionPlan::Type {
                newline_mode: NewlineMode::Raw,
                char_delay_ms: 0,
            }
        );
        c.requested = InsertionMethod::ClipboardOnly;
        assert_eq!(
            resolve_plan(&c),
            InsertionPlan::ClipboardOnly(ClipboardOnlyReason::Requested)
        );
    }

    /// The `paste` method honors the legacy chord selection, so a migrated
    /// `ctrl_shift_v` store keeps sending Ctrl+Shift+V.
    #[test]
    fn paste_delegates_the_chord_to_legacy_paste_method() {
        let mut c = ctx();
        c.requested = InsertionMethod::Paste;
        c.paste_method = PasteMethod::CtrlShiftV;
        assert_eq!(
            resolve_plan(&c),
            InsertionPlan::Paste(PasteMethod::CtrlShiftV)
        );
        // Non-chord legacy values fall back to Ctrl+V.
        c.paste_method = PasteMethod::Direct;
        assert_eq!(resolve_plan(&c), InsertionPlan::Paste(PasteMethod::CtrlV));
    }

    #[test]
    fn external_script_is_an_unconditional_escape_hatch() {
        let mut c = ctx();
        c.paste_method = PasteMethod::ExternalScript;
        c.target.elevated = true; // even under UIPI — the script decides
        assert_eq!(resolve_plan(&c), InsertionPlan::ExternalScript);
    }

    /// Spec edge case: explicit `type` with >5000 chars prefers paste;
    /// `auto`-resolved typing on a remote exe keeps typing (paste can't
    /// reach it anyway).
    #[test]
    fn long_text_prefers_paste_only_for_explicit_type() {
        let mut c = ctx();
        c.requested = InsertionMethod::Type;
        c.text_len = TYPE_LONG_TEXT_LEN + 1;
        assert_eq!(resolve_plan(&c), InsertionPlan::Paste(PasteMethod::CtrlV));

        c.requested = InsertionMethod::Auto;
        c.target.exe_name = Some("mstsc.exe".into());
        assert!(matches!(resolve_plan(&c), InsertionPlan::Type { .. }));
    }

    #[test]
    fn clipboard_only_never_sends_auto_submit() {
        assert!(!InsertionPlan::ClipboardOnly(ClipboardOnlyReason::Requested).sends_auto_submit());
        assert!(InsertionPlan::Paste(PasteMethod::CtrlV).sends_auto_submit());
        assert!(InsertionPlan::Type {
            newline_mode: NewlineMode::Raw,
            char_delay_ms: 0,
        }
        .sends_auto_submit());
    }

    // --- AC-005-07 / FR-005-05: newline_mode ----------------------------------

    #[test]
    fn raw_newlines_become_enter() {
        assert_eq!(
            type_segments("linha um\nlinha dois", NewlineMode::Raw),
            vec![
                TypeSegment::Text("linha um".into()),
                TypeSegment::Enter,
                TypeSegment::Text("linha dois".into()),
            ]
        );
    }

    #[test]
    fn shift_enter_mode_sends_shift_enter() {
        // AC-005-07: "linha um nova linha linha dois" → two lines, no submit.
        assert_eq!(
            type_segments("linha um\nlinha dois", NewlineMode::ShiftEnter),
            vec![
                TypeSegment::Text("linha um".into()),
                TypeSegment::ShiftEnter,
                TypeSegment::Text("linha dois".into()),
            ]
        );
    }

    #[test]
    fn crlf_counts_as_one_newline() {
        assert_eq!(
            type_segments("a\r\nb\rc\nd", NewlineMode::Raw),
            vec![
                TypeSegment::Text("a".into()),
                TypeSegment::Enter,
                TypeSegment::Text("b".into()),
                TypeSegment::Enter,
                TypeSegment::Text("c".into()),
                TypeSegment::Enter,
                TypeSegment::Text("d".into()),
            ]
        );
    }

    #[test]
    fn edge_cases_segment_cleanly() {
        assert!(type_segments("", NewlineMode::Raw).is_empty());
        assert_eq!(
            type_segments("\n", NewlineMode::Raw),
            vec![TypeSegment::Enter]
        );
        assert_eq!(
            type_segments("a\n\n", NewlineMode::ShiftEnter),
            vec![
                TypeSegment::Text("a".into()),
                TypeSegment::ShiftEnter,
                TypeSegment::ShiftEnter,
            ]
        );
    }

    #[test]
    fn method_names_match_history_vocabulary() {
        assert_eq!(
            InsertionPlan::Paste(PasteMethod::CtrlV).method_name(),
            "paste"
        );
        assert_eq!(
            InsertionPlan::Paste(PasteMethod::ShiftInsert).method_name(),
            "paste_shift_insert"
        );
        assert_eq!(
            InsertionPlan::Type {
                newline_mode: NewlineMode::Raw,
                char_delay_ms: 0,
            }
            .method_name(),
            "type"
        );
        assert_eq!(
            InsertionPlan::ClipboardOnly(ClipboardOnlyReason::ElevatedTarget).method_name(),
            "clipboard_only"
        );
    }
}
