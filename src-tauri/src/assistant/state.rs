//! Provider resolution + panel state snapshots (FR-012-04/17/18) and
//! dictation routing (FR-012-12) — everything that derives `assistant://state`
//! payloads or the session's routing flags from settings and the session.

use std::time::Duration;
use tauri::{AppHandle, Emitter};

use super::panel::focus_panel;
use super::{
    lock_session, AssistantPhase, AssistantProviderHint, AssistantSession, AssistantStateEvent,
    CachedProviderSnapshot, STATE_EVENT,
};
use crate::llm::cli_agent;
use crate::llm::router;
use crate::settings::{self, AppSettings, PostProcessProvider};
use crate::window_labels::ASSISTANT;

// ---------------------------------------------------------------------------
// Provider resolution (FR-012-04/17/18)
// ---------------------------------------------------------------------------

/// `(ready, hint)` for one provider — pure; the vault read happens in the
/// caller. Mirrors `commands::llm::summary_status`'s gating rules.
pub(crate) fn provider_status(
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
        let spec = cli_agent::adapter_for(&provider.id);
        let config = settings.cli_agent_config(&provider.id);
        if !config.enabled {
            return (false, Some(AssistantProviderHint::CliAgentDisabled));
        }
        let detected = spec
            .and_then(|spec| cli_agent::resolve_binary(spec, &config))
            .is_some();
        if !detected {
            return (false, Some(AssistantProviderHint::CliAgentNotDetected));
        }
        // Experimental adapters (no verified non-mutating headless mode) may
        // still be *explicitly* selected — the assistant is the interactive
        // surface where the user opted in, so the call is allowed and the
        // panel flags the provider as experimental. Auto-picks and
        // non-assistant purposes still refuse them (NFR-012-02).
        if spec.is_some_and(|s| s.experimental) {
            return (true, Some(AssistantProviderHint::CliAgentExperimental));
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
pub(crate) fn choose_provider(
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
            // Experimental adapters refuse `complete()` — never auto-pick one.
            !cli_agent::adapter_for(&p.id).is_some_and(|s| s.experimental)
                && cfg.enabled
                && cli_detected(&p.id)
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
        !spec.experimental && cfg.enabled && cli_detected(spec.provider_id)
    });
    if let Some(spec) = detected_cli {
        return settings.post_process_provider(spec.provider_id);
    }
    if byok.is_some() {
        return byok;
    }
    // Last resort for the hint: the first non-experimental CLI adapter's
    // provider row even when undetected — the panel can still point at
    // "install codex/claude".
    cli_agent::ADAPTERS
        .iter()
        .find(|spec| !spec.experimental)
        .and_then(|spec| settings.post_process_provider(spec.provider_id))
}

/// Settings+vault-facing wrapper around `choose_provider`.
pub(crate) fn resolve_provider(
    app: &AppHandle,
    settings: &AppSettings,
) -> Option<PostProcessProvider> {
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

/// How long a cached provider snapshot stays fresh. Resolving one reads the
/// OS keyring and scans PATH — too expensive for the hot paths that reach it
/// (every `emit_state`, every dictation start). Settings writes invalidate
/// instantly via the fingerprint; a newly installed binary or a key set
/// outside this session ages in within the TTL.
const PROVIDER_SNAPSHOT_TTL: Duration = Duration::from_secs(2);

/// Fingerprint of the settings fields that can change which provider answers
/// or whether it is ready — a write to any of them invalidates the cache.
fn provider_fingerprint(settings: &AppSettings) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let raw = serde_json::to_string(&(
        &settings.assistant_provider_id,
        &settings.post_process_provider_id,
        settings.offline_mode,
        &settings.post_process_models,
        &settings.cli_agent_configs,
        &settings.post_process_providers,
    ))
    .unwrap_or_default();
    let mut hasher = DefaultHasher::new();
    raw.hash(&mut hasher);
    hasher.finish()
}

/// The uncached resolution — keyring read + PATH scan + status derivation.
fn compute_provider_snapshot(
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

/// `compute_provider_snapshot` through the session's TTL+fingerprint cache.
/// Callers must already hold the session lock — which every emit path does
/// anyway for the ordering guarantee.
fn provider_snapshot(
    app: &AppHandle,
    settings: &AppSettings,
    session: &mut AssistantSession,
) -> (
    Option<PostProcessProvider>,
    bool,
    Option<AssistantProviderHint>,
) {
    let fingerprint = provider_fingerprint(settings);
    if let Some(cached) = session.provider_snapshot.as_ref() {
        if cached.fingerprint == fingerprint && cached.taken.elapsed() < PROVIDER_SNAPSHOT_TTL {
            return (cached.provider.clone(), cached.ready, cached.hint);
        }
    }
    let (provider, ready, hint) = compute_provider_snapshot(app, settings);
    session.provider_snapshot = Some(CachedProviderSnapshot {
        fingerprint,
        taken: std::time::Instant::now(),
        provider: provider.clone(),
        ready,
        hint,
    });
    (provider, ready, hint)
}

/// Build the panel state snapshot while holding `session` — provider fields
/// plus the session fields.
fn build_state_event(app: &AppHandle, session: &mut AssistantSession) -> AssistantStateEvent {
    let settings = settings::get_settings(app);
    let (provider, provider_ready, provider_hint) = provider_snapshot(app, &settings, session);
    AssistantStateEvent {
        open: session.open,
        phase: session.phase,
        dictating: session.dictating,
        provider_id: provider.as_ref().map(|p| p.id.clone()),
        provider_label: provider.as_ref().map(|p| p.label.clone()),
        provider_ready,
        provider_hint,
        queued_prompt: session.pending_prompt.clone(),
        error_kind: session.error_kind,
        error_detail: session.error_detail.clone(),
        messages: session.messages.clone(),
        pinned: settings.assistant_panel_pinned,
    }
}

/// Build + emit while the session guard is held: emissions are serialized by
/// the session lock, so a fast-finishing turn can never land a stale
/// `thinking` snapshot on top of the real outcome.
pub(crate) fn emit_state_locked(app: &AppHandle, session: &mut AssistantSession) {
    let event = build_state_event(app, session);
    let _ = app.emit_to(ASSISTANT, STATE_EVENT, event);
}

/// Full snapshot for `assistant_get_state` (webview hydration on mount).
pub fn state_event(app: &AppHandle) -> AssistantStateEvent {
    if let Some(mut session) = lock_session(app) {
        return build_state_event(app, &mut session);
    }
    // No session state yet — report a bare snapshot (still goes through the
    // uncached path: no session means nowhere to keep the cache).
    let settings = settings::get_settings(app);
    let (provider, provider_ready, provider_hint) = compute_provider_snapshot(app, &settings);
    AssistantStateEvent {
        open: false,
        phase: AssistantPhase::Idle,
        dictating: false,
        provider_id: provider.as_ref().map(|p| p.id.clone()),
        provider_label: provider.as_ref().map(|p| p.label.clone()),
        provider_ready,
        provider_hint,
        queued_prompt: None,
        error_kind: None,
        error_detail: None,
        messages: Vec::new(),
        pinned: settings.assistant_panel_pinned,
    }
}

pub(crate) fn emit_state(app: &AppHandle) {
    if let Some(mut session) = lock_session(app) {
        emit_state_locked(app, &mut session);
        return;
    }
    let _ = app.emit_to(ASSISTANT, STATE_EVENT, state_event(app));
}
// ---------------------------------------------------------------------------
// Dictation routing (FR-012-12)
// ---------------------------------------------------------------------------

/// Called by `TranscribeAction::start` for every dictation-family start.
/// Only the dedicated `assistant` binding is claimed — `transcribe` /
/// `transcribe_with_post_process` always dictate to the focused app, even
/// while the panel is open (FR-012-12). `stop` consumes the claim via
/// `take_dictation_route` and auto-sends instead of `paste_for_session`.
/// The claim is unconditional: the binding itself is explicit intent, and a
/// missing provider surfaces as a send error the panel explains.
pub fn maybe_claim_dictation(app: &AppHandle, binding_id: &str) {
    if binding_id != "assistant" {
        return;
    }
    let claimed = {
        let Some(mut session) = lock_session(app) else {
            return;
        };
        if !session.open || session.dictation_routed {
            return;
        }
        session.dictation_routed = true;
        session.dictating = true;
        emit_state_locked(app, &mut session);
        true
    };
    if claimed {
        // "um ditado pode focar o painel" (FR-012-11) — the user is talking
        // to the assistant, so the panel may take focus.
        focus_panel(app);
    }
}

/// Where a finished dictation's final text goes — consumed once, at `stop`.
/// The claim is set by `maybe_claim_dictation` when the `assistant` binding
/// started the capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DictationRoute {
    /// Never claimed — normal pipeline (paste into the focused app).
    Normal,
    /// Claimed, panel still open — auto-send the text to the provider.
    Send,
    /// Claimed but the panel was closed mid-dictation — drop the text.
    /// Pasting it into whatever happens to be focused would leak an
    /// assistant-bound dictation (closing the panel is the abort gesture).
    Drop,
}

/// Consume the dictation claim at `stop` time. `dictating` clears here so
/// the panel drops its live preview; the pipeline sends (`Send`), drops
/// (`Drop`), or pastes (`Normal`) accordingly — an assistant-claimed
/// session **never** reaches `paste_for_session`.
pub fn take_dictation_route(app: &AppHandle) -> DictationRoute {
    let Some(mut session) = lock_session(app) else {
        return DictationRoute::Normal;
    };
    let route = if !session.dictation_routed {
        DictationRoute::Normal
    } else if session.open {
        DictationRoute::Send
    } else {
        DictationRoute::Drop
    };
    if session.dictation_routed || session.dictating {
        session.dictation_routed = false;
        session.dictating = false;
        emit_state_locked(app, &mut session);
    }
    route
}

/// A claimed dictation that never reached `stop` (cancel, failed start) —
/// drop the claim so the next dictation isn't misrouted.
pub fn note_dictation_cancelled(app: &AppHandle) {
    let Some(mut session) = lock_session(app) else {
        return;
    };
    if session.dictation_routed || session.dictating {
        session.dictation_routed = false;
        session.dictating = false;
        emit_state_locked(app, &mut session);
    }
}
