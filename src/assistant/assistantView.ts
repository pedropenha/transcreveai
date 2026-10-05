/**
 * F012/T-091 — pure view-model for the assistant overlay panel.
 *
 * Framework-free so every transition can be asserted in plain Bun tests —
 * same pattern as `overlay/flowbarView.ts` and `toast/toastView.ts`. The DOM
 * component (`AssistantPanel.tsx`) only wires events and commands onto this.
 *
 * The panel is dictation-first (FR-012-12/13): `Ctrl+Shift+A` opens it and
 * starts a routed dictation; pressing again ends the dictation and the
 * backend auto-sends the transcript. The composer also accepts explicit
 * keyboard input without changing that voice routing.
 */

/** Whether the title strip can start a window drag (FR-012-16): the
 * docking toggle never prevents dragging to another edge. */
export function canDragPanel(_pinned: boolean): boolean {
  return true;
}

/** Which i18n key the pin toggle advertises (the action it will take, not
 * the current state). */
export function pinToggleKey(
  pinned: boolean,
): "assistant.pin" | "assistant.unpin" {
  return pinned ? "assistant.unpin" : "assistant.pin";
}

/** Pointer snapshot where a title-strip drag starts — `clientX/Y` is the
 * pointer's offset inside the window, which stays constant while the
 * window tracks the cursor (the backend subtracts it from `screenX/Y`). */
export interface PanelGrab {
  clientX: number;
  clientY: number;
}

/** True while `event` continues an active drag — guards every pointermove
 * without the component needing to keep the check inline. */
export function isPanelDragging(grab: PanelGrab | null): boolean {
  return grab !== null;
}

/** Advisory hints don't block usage — `cli_agent_experimental` marks a
 * provider the user explicitly selected; every other hint explains why the
 * provider can't answer. */
export function providerHintIsAdvisory(
  hint: string | null | undefined,
): boolean {
  return hint === "cli_agent_experimental";
}

/** snake_case `AssistantProviderHint` → the camelCase i18n key under
 * `assistant.hint.*`. `null`/unknown falls back to the generic line. */
export function providerHintKey(hint: string | null | undefined): string {
  switch (hint) {
    case "offline":
      return "assistant.hint.offline";
    case "missing_api_key":
      return "assistant.hint.missingApiKey";
    case "missing_model":
      return "assistant.hint.missingModel";
    case "cli_agent_disabled":
      return "assistant.hint.cliAgentDisabled";
    case "cli_agent_not_detected":
      return "assistant.hint.cliAgentNotDetected";
    case "cli_agent_experimental":
      return "assistant.hint.cliAgentExperimental";
    case "no_provider":
      return "assistant.hint.noProvider";
    default:
      return "assistant.hint.noProvider";
  }
}

/** snake_case `LlmErrorKind` → the localized `assistant.error.*` key.
 * The panel shows this as the primary error line — `errorDetail` (raw,
 * English, backend-generated) stays as secondary diagnostic text, never
 * as the user-facing message. `null`/unknown maps to the generic title. */
export function errorKindKey(kind: string | null | undefined): string {
  switch (kind) {
    case "network":
      return "assistant.error.network";
    case "timeout":
      return "assistant.error.timeout";
    case "auth":
      return "assistant.error.auth";
    case "rate_limited":
      return "assistant.error.rateLimited";
    case "unavailable":
      return "assistant.error.unavailable";
    case "missing_api_key":
      return "assistant.error.missingApiKey";
    case "offline":
      return "assistant.error.offline";
    case "unsupported":
      return "assistant.error.unsupported";
    default:
      return "assistant.error.provider";
  }
}

/** `react-markdown` `urlTransform`: only http/https/mailto links survive —
 * model output must never be able to smuggle `javascript:`/`data:`/`file:`
 * URLs into the panel's click-through `openUrl`. Anything else renders as
 * inert text. */
export function safeMarkdownUrl(url: string): string {
  const trimmed = url.trim();
  // Relative URLs and fragments carry no scheme → nothing external loads.
  const match = /^([a-zA-Z][a-zA-Z0-9+.-]*):/.exec(trimmed);
  if (!match) return url;
  const protocol = match[1].toLowerCase();
  return ["http", "https", "mailto"].includes(protocol) ? url : "";
}
