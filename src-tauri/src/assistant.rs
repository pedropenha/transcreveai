//! F012/T-091 — the floating voice-assistant overlay ("Assistente").
//!
//! A persistent hidden `assistant` webview — same eager-creation trick as the
//! Flow Bar and the meeting toast, since a cold `WebviewWindowBuilder` misses
//! the sub-second open budget of NFR-012-01 — that
//!
//! * appears topmost **without stealing focus** (FR-012-11): Windows shows it
//!   with `SetWindowPos(HWND_TOPMOST, SWP_NOACTIVATE | SWP_SHOWWINDOW)`, macOS
//!   uses a non-activating `NSPanel` (tauri-nspanel), Linux shows a regular
//!   always-on-top window as a best effort;
//! * gains keyboard focus only on explicit intent — a click
//!   (`assistant_focus` from the webview's mousedown) or an assistant-routed
//!   dictation (`maybe_claim_dictation`), per FR-012-11;
//! * keeps the multi-turn conversation in memory only (FR-012-15): nothing is
//!   persisted and nothing leaves the machine before an explicit send
//!   (FR-012-18);
//! * reserves a title-bar strip in the webview for T-092's drag/pin work —
//!   the window itself only needs to be (re)positioned on open here.
//!
//! Dictation routing (FR-012-12): a `transcribe*` press claims the session's
//! output for the panel while it is open (`dictation_routed`);
//! `actions::TranscribeAction::stop` consumes the claim and delivers the
//! final text over `assistant://dictated` instead of `paste_for_session` —
//! routed dictation is never inserted into another app and never runs the
//! LLM cleanup pass (it lands verbatim in the editable prompt field).
//!
//! Cancellation (AC-012-05): the in-flight call rides a spawned task whose
//! `JoinHandle` is aborted on cancel — for `cli_agent/*` providers the
//! `kill_on_drop` child dies with the future, and the generation guard drops
//! any result that lands after the cancel so no stale answer appears.

use serde::Serialize;
use specta::Type;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

use crate::commands::llm::LlmErrorKind;
use crate::commands::{CommandError, CommandErrorCode, CommandResult};
use crate::llm::cli_agent;
use crate::llm::router;
use crate::llm::types::{LlmMessage, LlmPurpose, LlmRequest, LlmResponse};
use crate::settings::{self, AppSettings, PostProcessProvider};
use crate::window_labels::ASSISTANT;

// ---------------------------------------------------------------------------
// IPC contract
// ---------------------------------------------------------------------------

/// Full panel snapshot — emitted on every state change and returned by
/// `assistant_get_state` so a cold webview hydrates on mount.
pub const STATE_EVENT: &str = "assistant://state";

/// Final text of an assistant-routed dictation — the panel appends it to the
/// editable prompt field (FR-012-12). `{ text }`.
pub const DICTATED_EVENT: &str = "assistant://dictated";

/// The global assistant hotkey was pressed while the panel was open — the
/// panel sends the draft when non-empty, else focuses the input (FR-012-10 /
/// FR-012-13: "segunda pressionada do atalho envia").
pub const HOTKEY_EVENT: &str = "assistant://hotkey";

/// How long a `pending_hotkey` press waits for a booting webview before it is
/// dropped — longer and a stale press could replay on an unrelated remount.
const PENDING_HOTKEY_TTL: Duration = Duration::from_secs(3);

// ---------------------------------------------------------------------------
// Panel geometry (logical px; physical conversion happens per platform)
// ---------------------------------------------------------------------------

/// Floating-panel footprint — generous enough for a chat exchange, compact
/// enough to read as an overlay. T-092 persists a dragged position; until
/// then every open recenters on the cursor's monitor.
pub(crate) const PANEL_WIDTH: f64 = 420.0;
pub(crate) const PANEL_HEIGHT: f64 = 560.0;
/// Bottom margin above the taskbar edge of the work area.
pub(crate) const PANEL_BOTTOM_MARGIN: f64 = 72.0;

/// Default upper bound on retained conversation messages (a turn costs two —
/// the user's prompt and the assistant's answer). Overridable for tests and
/// advanced tuning via `TRANSCREVE_ASSISTANT_HISTORY_LIMIT` (FR-012-15's
/// "limite configurável").
const DEFAULT_HISTORY_LIMIT: usize = 40;
const HISTORY_LIMIT_ENV: &str = "TRANSCREVE_ASSISTANT_HISTORY_LIMIT";

fn history_limit() -> usize {
    std::env::var(HISTORY_LIMIT_ENV)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_HISTORY_LIMIT)
}

/// Upper bound on a rendered assistant answer — the same bound the CLI-agent
/// providers enforce on their stdout (NFR-012-03). HTTP providers get the cap
/// applied here so the panel never has to layout an unbounded blob.
const MAX_RESPONSE_CHARS: usize = cli_agent::MAX_STDOUT_CHARS;
const TRUNCATED_MARKER: &str = "[response truncated]";

fn cap_response(text: String) -> String {
    if text.chars().count() <= MAX_RESPONSE_CHARS {
        return text;
    }
    let mut capped: String = text.chars().take(MAX_RESPONSE_CHARS).collect();
    capped.push_str("\n\n");
    capped.push_str(TRUNCATED_MARKER);
    capped
}

/// Turns sent to the provider and kept in the panel. FR-012-15: bounded
/// history — the oldest messages drop first.
fn truncate_history(messages: &mut Vec<AssistantMessage>) {
    let limit = history_limit();
    if messages.len() > limit {
        let overflow = messages.len() - limit;
        messages.drain(..overflow);
    }
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// One conversation turn as kept in memory and rendered by the panel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AssistantMessage {
    /// `"user"` | `"assistant"` — role strings, not the `LlmRole` enum, so the
    /// payload stays a plain string union in TypeScript.
    pub role: String,
    pub content: String,
}

impl AssistantMessage {
    fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: content.into(),
        }
    }
    fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.into(),
        }
    }
    fn to_llm(&self) -> Option<LlmMessage> {
        let role = match self.role.as_str() {
            "user" => crate::llm::types::LlmRole::User,
            "assistant" => crate::llm::types::LlmRole::Assistant,
            _ => return None,
        };
        Some(LlmMessage {
            role,
            content: self.content.clone(),
        })
    }
}

/// Panel lifecycle phase. `cancelled` is a resting state (the last in-flight
/// call was aborted); the next send/dictation moves on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum AssistantPhase {
    #[default]
    Idle,
    Thinking,
    Error,
    Cancelled,
}

/// Why `provider_ready` is false — stable snake_case tags the panel
/// localizes on (AC-012-06).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum AssistantProviderHint {
    /// No provider id could be resolved at all (no BYOK entry, no detected
    /// CLI agent).
    NoProvider,
    /// `settings.offline_mode` — no provider call is allowed.
    Offline,
    /// BYOK provider without a vaulted key.
    MissingApiKey,
    /// HTTP provider selected but no model configured.
    MissingModel,
    /// `cli_agent/*` provider with `enabled: false`.
    CliAgentDisabled,
    /// `cli_agent/*` provider enabled but the binary is not on PATH.
    CliAgentNotDetected,
}

/// Snapshot pushed to the panel on every transition.
#[derive(Clone, Debug, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AssistantStateEvent {
    pub open: bool,
    pub phase: AssistantPhase,
    /// A dictation session is currently routed to the panel — the panel
    /// shows the live STT preview (from `StreamTextEvent`) in the input.
    pub dictating: bool,
    /// A hotkey press arrived while the webview wasn't listening yet —
    /// consumed by `assistant_get_state`.
    pub pending_hotkey: bool,
    pub provider_id: Option<String>,
    pub provider_label: Option<String>,
    /// The provider is usable right now — gates dictation claims *and* the
    /// send button (AC-012-06).
    pub provider_ready: bool,
    pub provider_hint: Option<AssistantProviderHint>,
    /// Last provider failure, classified for localization.
    pub error_kind: Option<LlmErrorKind>,
    pub error_detail: Option<String>,
    pub messages: Vec<AssistantMessage>,
}

#[derive(Default)]
struct AssistantSession {
    open: bool,
    phase: AssistantPhase,
    /// This dictation session's output belongs to the panel — set at
    /// recording start (panel open), consumed at stop (FR-012-12).
    dictation_routed: bool,
    dictating: bool,
    /// Instant the hotkey press was recorded — the flag expires after
    /// `PENDING_HOTKEY_TTL` so a press that a live webview already consumed
    /// can't be replayed by a later remount (explicit-send only, FR-012-13).
    pending_hotkey: bool,
    pending_hotkey_at: Option<std::time::Instant>,
    error_kind: Option<LlmErrorKind>,
    error_detail: Option<String>,
    messages: Vec<AssistantMessage>,
    /// Monotonic turn counter — a response whose generation no longer
    /// matches was cancelled/superseded and is dropped.
    generation: u64,
    in_flight: Option<tauri::async_runtime::JoinHandle<()>>,
}

fn lock_session(app: &AppHandle) -> Option<std::sync::MutexGuard<'_, AssistantSession>> {
    let state = app.try_state::<Mutex<AssistantSession>>()?;
    // `inner()` reborrows the managed Mutex with `app`'s lifetime so the
    // guard can leave this function.
    Some(state.inner().lock().unwrap_or_else(|e| e.into_inner()))
}

// ---------------------------------------------------------------------------
// Provider resolution (FR-012-04/17/18)
// ---------------------------------------------------------------------------

/// `(ready, hint)` for one provider — pure; the vault read happens in the
/// caller. Mirrors `commands::llm::summary_status`'s gating rules.
fn provider_status(
    settings: &AppSettings,
    provider: Option<&PostProcessProvider>,
    has_api_key: bool,
) -> (bool, Option<AssistantProviderHint>) {
    let Some(provider) = provider else {
        return (false, Some(AssistantProviderHint::NoProvider));
    };
    if settings.offline_mode {
        return (false, Some(AssistantProviderHint::Offline));
    }
    if cli_agent::is_cli_agent(&provider.id) {
        let config = settings.cli_agent_config(&provider.id);
        if !config.enabled {
            return (false, Some(AssistantProviderHint::CliAgentDisabled));
        }
        let detected = cli_agent::adapter_for(&provider.id)
            .and_then(|spec| cli_agent::resolve_binary(spec, &config))
            .is_some();
        if !detected {
            return (false, Some(AssistantProviderHint::CliAgentNotDetected));
        }
        // CLI agents need no model — empty means the CLI's own default.
        return (true, None);
    }
    if router::requires_api_key(provider) && !has_api_key {
        return (false, Some(AssistantProviderHint::MissingApiKey));
    }
    let model = settings
        .post_process_models
        .get(&provider.id)
        .map(|m| m.trim().is_empty())
        .unwrap_or(true);
    if model {
        return (false, Some(AssistantProviderHint::MissingModel));
    }
    (true, None)
}

/// Which provider answers the assistant. `has_api_key`/`cli_detected` are
/// injected so the function stays pure and unit-testable.
fn choose_provider(
    settings: &AppSettings,
    has_api_key: impl Fn(&str) -> bool,
    cli_detected: impl Fn(&str) -> bool,
) -> Option<&PostProcessProvider> {
    // Explicit choice wins — even when currently unusable, so the panel can
    // say *why* (AC-012-06).
    if let Some(id) = settings.assistant_provider_id.as_deref() {
        return settings.post_process_provider(id);
    }
    // Auto: a usable BYOK provider first (it is the configured choice), then
    // the first detected+enabled CLI agent (FR-012-04 "auto-detect"). An
    // unusable BYOK is still returned over "nothing" so the hint explains the
    // fix instead of showing a bare empty state.
    let byok = settings.active_post_process_provider();
    let byok_ready = byok.is_some_and(|p| {
        if cli_agent::is_cli_agent(&p.id) {
            let cfg = settings.cli_agent_config(&p.id);
            cfg.enabled && cli_detected(&p.id)
        } else if router::requires_api_key(p) {
            has_api_key(&p.id)
                && settings
                    .post_process_models
                    .get(&p.id)
                    .is_some_and(|m| !m.trim().is_empty())
        } else {
            settings
                .post_process_models
                .get(&p.id)
                .is_some_and(|m| !m.trim().is_empty())
        }
    });
    if byok_ready {
        return byok;
    }
    let detected_cli = cli_agent::ADAPTERS.iter().find(|spec| {
        let cfg = settings.cli_agent_config(spec.provider_id);
        cfg.enabled && cli_detected(spec.provider_id)
    });
    if let Some(spec) = detected_cli {
        return settings.post_process_provider(spec.provider_id);
    }
    if byok.is_some() {
        return byok;
    }
    // Last resort for the hint: the first CLI adapter's provider row even
    // when undetected — the panel can still point at "install codex/claude".
    cli_agent::ADAPTERS
        .first()
        .and_then(|spec| settings.post_process_provider(spec.provider_id))
}

/// Settings+vault-facing wrapper around `choose_provider`.
fn resolve_provider(app: &AppHandle, settings: &AppSettings) -> Option<PostProcessProvider> {
    choose_provider(
        settings,
        |id| {
            crate::secrets::provider_api_key(app, id)
                .map(|k| !k.trim().is_empty())
                .unwrap_or(false)
        },
        |id| {
            cli_agent::adapter_for(id).is_some_and(|spec| {
                let config = settings.cli_agent_config(spec.provider_id);
                cli_agent::resolve_binary(spec, &config).is_some()
            })
        },
    )
    .cloned()
}

fn provider_snapshot(
    app: &AppHandle,
    settings: &AppSettings,
) -> (
    Option<PostProcessProvider>,
    bool,
    Option<AssistantProviderHint>,
) {
    let provider = resolve_provider(app, settings);
    let has_key = provider
        .as_ref()
        .map(|p| {
            crate::secrets::provider_api_key(app, &p.id)
                .map(|k| !k.trim().is_empty())
                .unwrap_or(false)
        })
        .unwrap_or(false);
    let (ready, hint) = provider_status(settings, provider.as_ref(), has_key);
    (provider, ready, hint)
}

/// Build the panel state snapshot. `consume_pending_hotkey` (the
/// `assistant_get_state` hydration path) clears the flag so a press is
/// delivered exactly once; event emissions keep it so a webview still
/// booting can pick the press up later.
pub fn state_event(app: &AppHandle, consume_pending_hotkey: bool) -> AssistantStateEvent {
    let settings = settings::get_settings(app);
    let (provider, provider_ready, provider_hint) = provider_snapshot(app, &settings);
    let mut event = AssistantStateEvent {
        open: false,
        phase: AssistantPhase::Idle,
        dictating: false,
        pending_hotkey: false,
        provider_id: provider.as_ref().map(|p| p.id.clone()),
        provider_label: provider.as_ref().map(|p| p.label.clone()),
        provider_ready,
        provider_hint,
        error_kind: None,
        error_detail: None,
        messages: Vec::new(),
    };
    if let Some(mut session) = lock_session(app) {
        // Expire a press nobody consumed — it belonged to a webview that was
        // already live (handled via HOTKEY_EVENT) or one that never booted.
        if session.pending_hotkey
            && session
                .pending_hotkey_at
                .is_some_and(|at| at.elapsed() > PENDING_HOTKEY_TTL)
        {
            session.pending_hotkey = false;
            session.pending_hotkey_at = None;
        }
        event.open = session.open;
        event.phase = session.phase;
        event.dictating = session.dictating;
        event.pending_hotkey = session.pending_hotkey;
        event.error_kind = session.error_kind;
        event.error_detail = session.error_detail.clone();
        event.messages = session.messages.clone();
        if consume_pending_hotkey {
            session.pending_hotkey = false;
            session.pending_hotkey_at = None;
        }
    }
    event
}

pub(crate) fn emit_state(app: &AppHandle) {
    let event = state_event(app, false);
    let _ = app.emit_to(ASSISTANT, STATE_EVENT, event);
}

// ---------------------------------------------------------------------------
// Window (FR-012-11)
// ---------------------------------------------------------------------------

/// Panel origin in the monitor's **physical** pixels: centered horizontally,
/// docked `PANEL_BOTTOM_MARGIN` (logical) above the work-area bottom. Pure
/// math — `work_area` is physical px, `scale` the monitor's DPI factor
/// (times the Windows accessibility text scale on that platform).
fn panel_origin(work_area: (i32, i32, u32, u32), scale: f64) -> (f64, f64) {
    let (ax, ay, aw, ah) = work_area;
    let x = ax as f64 + (aw as f64 - PANEL_WIDTH * scale) / 2.0;
    let y = ay as f64 + ah as f64 - (PANEL_HEIGHT + PANEL_BOTTOM_MARGIN) * scale;
    (x.max(0.0), y.max(0.0))
}

/// Logical-point origin for the platforms where Tauri positions windows in
/// logical coordinates (macOS, Linux).
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn panel_logical_origin(app: &AppHandle) -> Option<(f64, f64)> {
    let monitor = crate::overlay::get_monitor_with_cursor(app)?;
    let wa = monitor.work_area();
    let scale = monitor.scale_factor();
    let (x, y) = panel_origin(
        (wa.position.x, wa.position.y, wa.size.width, wa.size.height),
        scale,
    );
    Some((x / scale, y / scale))
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
    if let Some(monitor) = crate::overlay::get_monitor_with_cursor(app) {
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
    emit_state(app);
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || show_native(&handle));
}

/// Hide the panel; the session (history, draft-independent state) survives —
/// the conversation is cleared only by `new_conversation` (FR-012-15).
pub fn close_panel(app: &AppHandle) {
    {
        let Some(mut session) = lock_session(app) else {
            return;
        };
        session.open = false;
    }
    emit_state(app);
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || hide_native(&handle));
}

/// The OS-level close path (`on_window_event` hides the window itself) —
/// keep the session flag honest so the next hotkey press re-opens instead of
/// sending.
pub fn note_panel_hidden(app: &AppHandle) {
    let changed = {
        let Some(mut session) = lock_session(app) else {
            return;
        };
        let was = session.open;
        session.open = false;
        was
    };
    if changed {
        emit_state(app);
    }
}

/// Explicit user intent to interact — click (`assistant_focus`) or a routed
/// dictation start (`maybe_claim_dictation`).
pub fn focus_panel(app: &AppHandle) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || focus_native(&handle));
}

/// The global assistant hotkey. First press opens; with the panel open the
/// press is forwarded to the webview (`assistant://hotkey`), which sends a
/// non-empty draft or focuses the input (FR-012-10 / FR-012-13).
pub fn hotkey_pressed(app: &AppHandle) {
    let open = lock_session(app).map(|s| s.open).unwrap_or(false);
    if !open {
        open_panel(app);
        return;
    }
    if let Some(mut session) = lock_session(app) {
        // Recorded so a webview still booting can pick it up via
        // `assistant_get_state` instead of losing the press.
        session.pending_hotkey = true;
        session.pending_hotkey_at = Some(std::time::Instant::now());
    }
    let _ = app.emit_to(ASSISTANT, HOTKEY_EVENT, ());
}

// ---------------------------------------------------------------------------
// Dictation routing (FR-012-12)
// ---------------------------------------------------------------------------

/// Called by `TranscribeAction::start` when a `transcribe*` recording begins.
/// With the panel open the session's output is claimed for the assistant
/// input — `stop` consumes the claim via `take_dictation_route` and skips
/// `paste_for_session` entirely. AC-012-06: no usable provider → no claim,
/// so the dictation keeps its normal destination instead of being
/// transcribed "para envio".
pub fn maybe_claim_dictation(app: &AppHandle) {
    let settings = settings::get_settings(app);
    let (_provider, provider_ready, _hint) = provider_snapshot(app, &settings);
    if !provider_ready {
        return;
    }
    let changed = {
        let Some(mut session) = lock_session(app) else {
            return;
        };
        if !session.open || session.dictation_routed {
            false
        } else {
            session.dictation_routed = true;
            session.dictating = true;
            true
        }
    };
    if changed {
        emit_state(app);
        // "um ditado pode focar o painel" (FR-012-11) — the user is talking
        // to the assistant, so the panel may take focus for quick edits.
        focus_panel(app);
    }
}

/// Consume the dictation claim at `stop` time. Returns true when this
/// session's final text must go to the panel instead of any external app.
/// `dictating` clears here so the panel drops its live preview; the final
/// text follows on `assistant://dictated`.
pub fn take_dictation_route(app: &AppHandle) -> bool {
    let claimed = {
        let Some(mut session) = lock_session(app) else {
            return false;
        };
        let claimed = session.dictation_routed;
        if claimed {
            session.dictation_routed = false;
            session.dictating = false;
        }
        claimed
    };
    if claimed {
        emit_state(app);
    }
    claimed
}

/// A claimed dictation that never reached `stop` (cancel, failed start) —
/// drop the claim so the next dictation isn't misrouted.
pub fn note_dictation_cancelled(app: &AppHandle) {
    let changed = {
        let Some(mut session) = lock_session(app) else {
            return;
        };
        let changed = session.dictation_routed || session.dictating;
        session.dictation_routed = false;
        session.dictating = false;
        changed
    };
    if changed {
        emit_state(app);
    }
}

/// Deliver the finalized dictation text to the panel input.
pub fn deliver_dictated_text(app: &AppHandle, text: &str) {
    #[derive(Serialize, Clone)]
    struct DictatedPayload<'a> {
        text: &'a str,
    }
    let _ = app.emit_to(ASSISTANT, DICTATED_EVENT, DictatedPayload { text });
}

// ---------------------------------------------------------------------------
// Turns
// ---------------------------------------------------------------------------

const SYSTEM_PROMPT: &str = "You are the Transcreve.ai voice assistant — a concise, helpful \
assistant the user talks to from a floating panel, often by speech. \
Keep answers short unless detail is asked for, use Markdown sparingly, \
and reply in the language the user writes or dictates in.";

fn short_error(err: &impl std::fmt::Display) -> String {
    err.to_string().chars().take(160).collect()
}

/// Spawn the provider call for `history` (already truncated). The result is
/// committed only if the generation still matches — a cancel or a new
/// conversation bumps it, dropping stale/partial answers (AC-012-05).
fn spawn_turn(app: &AppHandle, history: Vec<AssistantMessage>, generation: u64) {
    let handle = tauri::async_runtime::spawn(run_turn(app.clone(), history, generation));
    if let Some(mut session) = lock_session(app) {
        session.in_flight = Some(handle);
    }
}

async fn run_turn(app: AppHandle, history: Vec<AssistantMessage>, generation: u64) {
    let outcome = complete_turn(&app, &history).await;
    let emit = {
        let Some(mut session) = lock_session(&app) else {
            return;
        };
        if session.generation != generation {
            // Cancelled or superseded — never surface a stale answer.
            return;
        }
        session.in_flight = None;
        match outcome {
            Ok(response) => {
                session
                    .messages
                    .push(AssistantMessage::assistant(response.text));
                truncate_history(&mut session.messages);
                session.phase = AssistantPhase::Idle;
                session.error_kind = None;
                session.error_detail = None;
            }
            Err(e) => {
                log::warn!("assistant turn failed: {e}");
                session.phase = AssistantPhase::Error;
                session.error_kind = Some(LlmErrorKind::from(&e));
                session.error_detail = Some(short_error(&e));
            }
        }
        true
    };
    if emit {
        emit_state(&app);
    }
}

/// Provider call for one turn: resolve → route → complete with the narrow
/// transient-retry policy. `cli_agent/*` rides `resolve_route`'s keyless path
/// (NFR-012-02 stays inside `CliAgentProvider`: argv, no shell, kill, cap).
async fn complete_turn(
    app: &AppHandle,
    history: &[AssistantMessage],
) -> Result<LlmResponse, crate::llm::types::LlmError> {
    use crate::llm::types::LlmError;
    let settings = settings::get_settings(app);
    let provider = resolve_provider(app, &settings)
        .ok_or_else(|| LlmError::Provider("no assistant provider configured".to_string()))?;
    let api_key = crate::secrets::provider_api_key(app, &provider.id);
    let mut messages: Vec<LlmMessage> = Vec::with_capacity(history.len());
    for m in history {
        if let Some(msg) = m.to_llm() {
            messages.push(msg);
        }
    }
    let req = LlmRequest {
        system: SYSTEM_PROMPT.to_string(),
        messages,
        max_tokens: 2048,
        temperature: 0.4,
        timeout: Duration::from_secs(120),
        purpose: LlmPurpose::Assistant,
    };
    let input_chars = req.input_chars();
    let route = router::resolve_route_for_provider(
        &settings,
        provider,
        api_key,
        LlmPurpose::Assistant,
        input_chars,
    )?;
    let llm = router::build_provider(&route)?;
    router::complete_with_retry(llm.as_ref(), &req)
        .await
        .map(|mut r| {
            r.text = cap_response(std::mem::take(&mut r.text));
            r
        })
}

/// `assistant_send` — push the user message and kick the provider call.
/// FR-012-13: sending is always an explicit action; nothing is sent before.
pub fn send(app: &AppHandle, prompt: &str) -> CommandResult<()> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err(CommandError::new(
            CommandErrorCode::InvalidInput,
            "Empty prompt",
        ));
    }
    let (history, generation) = {
        let Some(mut session) = lock_session(app) else {
            return Err(CommandError::new(
                CommandErrorCode::Internal,
                "Assistant is not initialized",
            ));
        };
        if session.in_flight.is_some() {
            return Err(CommandError::new(
                CommandErrorCode::Busy,
                "The assistant is still answering",
            ));
        }
        session.phase = AssistantPhase::Thinking;
        session.error_kind = None;
        session.error_detail = None;
        session.messages.push(AssistantMessage::user(prompt));
        truncate_history(&mut session.messages);
        session.generation += 1;
        (session.messages.clone(), session.generation)
    };
    spawn_turn(app, history, generation);
    emit_state(app);
    Ok(())
}

/// `assistant_retry` — re-run the last call after a failure. The failed
/// user message stays last in history, so the retry is just a respawn.
pub fn retry(app: &AppHandle) -> CommandResult<()> {
    let (history, generation) = {
        let Some(mut session) = lock_session(app) else {
            return Err(CommandError::new(
                CommandErrorCode::Internal,
                "Assistant is not initialized",
            ));
        };
        if session.in_flight.is_some() {
            return Err(CommandError::new(
                CommandErrorCode::Busy,
                "The assistant is still answering",
            ));
        }
        if session.phase != AssistantPhase::Error
            || session.messages.last().map(|m| m.role.as_str()) != Some("user")
        {
            return Err(CommandError::new(
                CommandErrorCode::InvalidInput,
                "Nothing to retry",
            ));
        }
        session.phase = AssistantPhase::Thinking;
        session.error_kind = None;
        session.error_detail = None;
        session.generation += 1;
        (session.messages.clone(), session.generation)
    };
    spawn_turn(app, history, generation);
    emit_state(app);
    Ok(())
}

/// Abort the in-flight call. The JoinHandle abort drops the provider future;
/// for `cli_agent/*` the `kill_on_drop` child dies with it (AC-012-05).
pub fn cancel_in_flight(app: &AppHandle) -> bool {
    let cancelled = {
        let Some(mut session) = lock_session(app) else {
            return false;
        };
        match session.in_flight.take() {
            Some(handle) => {
                handle.abort();
                session.generation += 1;
                session.phase = AssistantPhase::Cancelled;
                true
            }
            None => false,
        }
    };
    if cancelled {
        emit_state(app);
    }
    cancelled
}

/// Esc from the panel: while thinking it cancels (panel stays open with the
/// "cancelled" state); otherwise it closes the panel.
pub fn dismiss(app: &AppHandle) {
    if !cancel_in_flight(app) {
        close_panel(app);
    }
}

/// "Nova conversa" — abort any in-flight call and clear history (FR-012-15).
pub fn new_conversation(app: &AppHandle) {
    {
        let Some(mut session) = lock_session(app) else {
            return;
        };
        if let Some(handle) = session.in_flight.take() {
            handle.abort();
        }
        session.generation += 1;
        session.messages.clear();
        session.phase = AssistantPhase::Idle;
        session.error_kind = None;
        session.error_detail = None;
    }
    emit_state(app);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::CliAgentConfig;

    fn msg(role: &str, content: impl Into<String>) -> AssistantMessage {
        AssistantMessage {
            role: role.to_string(),
            content: content.into(),
        }
    }

    fn provider(id: &str) -> PostProcessProvider {
        PostProcessProvider {
            id: id.to_string(),
            label: id.to_string(),
            base_url: format!("https://example.com/{id}"),
            allow_base_url_edit: true,
            models_endpoint: Some("/models".to_string()),
            supports_structured_output: false,
        }
    }

    #[test]
    fn truncate_history_keeps_the_newest_messages() {
        let mut messages: Vec<AssistantMessage> = (0..60)
            .map(|i| {
                msg(
                    if i % 2 == 0 { "user" } else { "assistant" },
                    format!("m{i}"),
                )
            })
            .collect();
        truncate_history(&mut messages);
        assert_eq!(messages.len(), DEFAULT_HISTORY_LIMIT);
        assert_eq!(messages.last().unwrap().content, "m59");
        assert_eq!(messages.first().unwrap().content, "m20");
    }

    #[test]
    fn cap_response_marks_truncation() {
        let long = "x".repeat(MAX_RESPONSE_CHARS + 10);
        let capped = cap_response(long);
        assert!(capped.ends_with(TRUNCATED_MARKER));
        assert!(capped.chars().count() <= MAX_RESPONSE_CHARS + TRUNCATED_MARKER.len() + 2);
    }

    #[test]
    fn provider_status_explains_each_unready_cause() {
        let mut settings = AppSettings::default();
        settings
            .post_process_models
            .insert("openai".to_string(), "gpt".to_string());
        let openai = provider("openai");

        // No provider at all.
        assert_eq!(
            provider_status(&settings, None, false),
            (false, Some(AssistantProviderHint::NoProvider))
        );
        // BYOK without key / with key.
        assert_eq!(
            provider_status(&settings, Some(&openai), false),
            (false, Some(AssistantProviderHint::MissingApiKey))
        );
        assert_eq!(
            provider_status(&settings, Some(&openai), true),
            (true, None)
        );
        // Offline gates even a fully configured provider.
        settings.offline_mode = true;
        assert_eq!(
            provider_status(&settings, Some(&openai), true),
            (false, Some(AssistantProviderHint::Offline))
        );
        settings.offline_mode = false;

        // Missing model.
        settings
            .post_process_models
            .insert("openai".into(), String::new());
        assert_eq!(
            provider_status(&settings, Some(&openai), true),
            (false, Some(AssistantProviderHint::MissingModel))
        );
    }

    #[test]
    fn cli_agent_status_gates_on_enabled_and_detection() {
        let settings = AppSettings::default();
        let codex = provider("cli_agent/codex");
        // Machine-dependent detection — pin the disabled arm and the
        // enabled-but-undetected arm only when no codex is on PATH.
        let mut disabled = settings.clone();
        disabled.cli_agent_configs.insert(
            "cli_agent/codex".to_string(),
            CliAgentConfig {
                enabled: false,
                ..CliAgentConfig::default()
            },
        );
        assert_eq!(
            provider_status(&disabled, Some(&codex), false),
            (false, Some(AssistantProviderHint::CliAgentDisabled))
        );
    }

    #[test]
    fn choose_provider_prefers_explicit_setting() {
        let mut settings = AppSettings {
            post_process_provider_id: "openai".to_string(),
            assistant_provider_id: Some("cli_agent/claude".to_string()),
            ..Default::default()
        };
        settings
            .post_process_models
            .insert("openai".to_string(), "gpt".to_string());
        let chosen = choose_provider(&settings, |_| true, |_| true);
        assert_eq!(chosen.map(|p| p.id.as_str()), Some("cli_agent/claude"));
    }

    #[test]
    fn choose_provider_auto_prefers_usable_byok_then_detected_cli() {
        let mut settings = AppSettings {
            post_process_provider_id: "openai".to_string(),
            ..Default::default()
        };
        settings
            .post_process_models
            .insert("openai".to_string(), "gpt".to_string());
        // Usable BYOK wins over a detected CLI agent.
        let chosen = choose_provider(&settings, |_| true, |_| true);
        assert_eq!(chosen.map(|p| p.id.as_str()), Some("openai"));
        // No key → the detected CLI agent takes over.
        let chosen = choose_provider(&settings, |_| false, |_| true);
        assert_eq!(chosen.map(|p| p.id.as_str()), Some("cli_agent/codex"));
        // Nothing usable → BYOK still surfaces so the hint explains the fix.
        let chosen = choose_provider(&settings, |_| false, |_| false);
        assert_eq!(chosen.map(|p| p.id.as_str()), Some("openai"));
    }

    #[test]
    fn panel_origin_centers_and_docks_above_the_work_area() {
        // 1920x1080 work area at 1x scale — physical px out.
        let (x, y) = panel_origin((0, 0, 1920, 1080), 1.0);
        assert_eq!(x, (1920.0 - PANEL_WIDTH) / 2.0);
        assert_eq!(y, 1080.0 - PANEL_HEIGHT - PANEL_BOTTOM_MARGIN);
        // At 2x scale the same logical rect lands on doubled physical coords.
        let (x2, y2) = panel_origin((0, 0, 3840, 2160), 2.0);
        assert_eq!(x2, (3840.0 - PANEL_WIDTH * 2.0) / 2.0);
        assert_eq!(y2, 2160.0 - (PANEL_HEIGHT + PANEL_BOTTOM_MARGIN) * 2.0);
        // Short work areas clamp at the top edge.
        let (_x, y3) = panel_origin((0, 0, 800, 400), 1.0);
        assert_eq!(y3, 0.0);
    }
}
