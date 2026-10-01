/**
 * F012/T-091 — pure view-model for the assistant overlay panel.
 *
 * Framework-free so every transition can be asserted in plain Bun tests —
 * same pattern as `overlay/flowbarView.ts` and `toast/toastView.ts`. The DOM
 * component (`AssistantPanel.tsx`) only wires events and commands onto this.
 */

/** What the second hotkey press does when the panel is already open
 * (FR-012-13: "a segunda pressionada do atalho envia"). */
export type HotkeyIntent = "send" | "focus" | "ignore";

export interface AssistantViewState {
  phase: "idle" | "thinking" | "error" | "cancelled";
  providerReady: boolean;
}

/** Append finalized dictation to the editable draft (FR-012-12): a typed
 * draft and a dictated fragment join with a single space, and an empty
 * dictated payload never clobbers what the user typed. */
export function appendDictated(draft: string, text: string): string {
  const fragment = text.trim();
  if (!fragment) return draft;
  const head = draft.trimEnd();
  return head ? `${head} ${fragment}` : fragment;
}

/** While a routed dictation is live the input shows draft + live STT
 * preview (readonly); once `assistant://dictated` lands the text is folded
 * into the real draft. */
export function composerValue(
  draft: string,
  liveText: string,
  dictating: boolean,
): string {
  return dictating && liveText ? appendDictated(draft, liveText) : draft;
}

/** A send is only ever explicit (FR-012-13) and only with a usable provider
 * (AC-012-06: the no-provider state cannot submit). */
export function canSend(draft: string, state: AssistantViewState): boolean {
  return (
    state.providerReady && state.phase !== "thinking" && draft.trim() !== ""
  );
}

/** Second press of the assistant hotkey with the panel open: a non-empty
 * draft sends (when a provider can answer), an empty draft focuses the
 * input, and mid-call presses are ignored. */
export function hotkeyIntent(
  draft: string,
  state: AssistantViewState,
): HotkeyIntent {
  if (state.phase === "thinking") return "ignore";
  if (!canSend(draft, state)) return "focus";
  return "send";
}

/** Whether the title strip can start a window drag (FR-012-16): the
 * "Fixar" toggle locks the panel — visible but immovable. */
export function canDragPanel(pinned: boolean): boolean {
  return !pinned;
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
    case "no_provider":
      return "assistant.hint.noProvider";
    default:
      return "assistant.hint.noProvider";
  }
}
