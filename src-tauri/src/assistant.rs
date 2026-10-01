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
//! * is draggable by its title strip and pinnable (T-092, FR-012-16): the
//!   webview reports pointer positions, this module clamps them onto the
//!   monitor under the cursor, and the position/pin land in settings so the
//!   panel reopens where it was left (AC-012-04).
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
use tauri::{AppHandle, Manager};

use crate::commands::llm::LlmErrorKind;
use crate::llm::types::LlmMessage;
use crate::settings::PostProcessProvider;

mod panel;
mod state;
#[cfg(test)]
mod tests;
mod turn;

pub use panel::{
    close_panel, focus_panel, hotkey_pressed, init, move_panel, note_panel_hidden, open_panel,
};
pub(crate) use state::emit_state;
pub use state::{
    deliver_dictated_text, maybe_claim_dictation, note_dictation_cancelled, state_event,
    take_dictation_route,
};
pub use turn::{cancel_in_flight, dismiss, new_conversation, retry, send};

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
    /// `cli_agent/*` adapter without a verified non-mutating headless mode —
    /// refused outright (NFR-012-02), no matter its config or detection.
    CliAgentExperimental,
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
    /// FR-012-16: the persisted "Fixar" toggle — while true the title strip
    /// shows the pinned state and drags are ignored.
    pub pinned: bool,
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
    /// Cached provider resolution — reads the OS keyring and scans PATH, so
    /// hot paths reuse it within [`PROVIDER_SNAPSHOT_TTL`].
    provider_snapshot: Option<CachedProviderSnapshot>,
}

struct CachedProviderSnapshot {
    fingerprint: u64,
    taken: std::time::Instant,
    provider: Option<PostProcessProvider>,
    ready: bool,
    hint: Option<AssistantProviderHint>,
}

fn lock_session(app: &AppHandle) -> Option<std::sync::MutexGuard<'_, AssistantSession>> {
    let state = app.try_state::<Mutex<AssistantSession>>()?;
    // `inner()` reborrows the managed Mutex with `app`'s lifetime so the
    // guard can leave this function.
    Some(state.inner().lock().unwrap_or_else(|e| e.into_inner()))
}
