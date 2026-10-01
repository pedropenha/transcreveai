/**
 * Pure view model for the meeting toast (F008 / T-062): which face the toast
 * window shows, given the backend's `toast://state` broadcast, pointer hover,
 * the ▾ menu and the post-start confirmation. Framework-free so every
 * transition is unit-tested without a DOM — the FR-008-07..10 evidence.
 *
 * State source:
 * - `toast://state` (toast.rs, broadcast to every window):
 *   `{ collapsed, detection, notice }`. `collapsed: true` means the window is
 *   hidden and the Flow Bar shows the amber reopen dot instead.
 */

/** A meeting detection forwarded by `detector://meeting`. */
export interface MeetingDetection {
  detection_id: string;
  app_label: string;
  exe: string;
  /** Rule action that produced the detection (`ask`, `auto_start`, …). */
  action?: string | null;
  started_at?: string | number | null;
}

/** A generic `toast://show` warning/notice. */
export interface ToastNotice {
  kind: string;
  message: string;
  action?: unknown;
}

/** `toast://state` payload — all keys always present (null when absent). */
export interface ToastStateEvent {
  collapsed: boolean;
  detection: MeetingDetection | null;
  notice: ToastNotice | null;
}

/** The toast's visual classes (FR-008-07..09). */
export type ToastView =
  | "hidden" // nothing to show, or collapsed to the amber dot
  | "compact" // icon + "Reunião detectada" + "● Agora" + app (FR-008-07)
  | "expanded" // hover: ✕ + "Iniciar Notetaker" + ▾ menu (FR-008-08)
  | "confirming" // "Gravando · <App>" for 3 s after start (FR-008-09)
  | "notice" // generic compact informational toast (toast://show)
  | "notice-expanded"; // hovered notice (✕ dismiss visible)

export interface ToastViewInput {
  /** Latest `toast://state`. */
  state: ToastStateEvent;
  /** Pointer is over the card (window is always interactive — no
   *  click-through like the Flow Bar). */
  hovered: boolean;
  /** The ▾ action menu is open. */
  menuOpen: boolean;
  /** App label while the "Gravando · <App>" confirmation is up. */
  confirming: string | null;
}

export function resolveToastView(input: ToastViewInput): ToastView {
  const { state, hovered, menuOpen, confirming } = input;

  if (state.detection === null && state.notice === null) return "hidden";
  // Collapsed = the amber dot on the Flow Bar is the whole UI (FR-008-10/12).
  if (state.collapsed) return "hidden";
  // The confirmation outranks expansion: after "Iniciar Notetaker" lands the
  // toast morphs to "Gravando · <App>" regardless of where the cursor sits.
  if (confirming !== null && state.detection !== null) return "confirming";
  if (state.detection !== null) {
    return hovered || menuOpen ? "expanded" : "compact";
  }
  // Notices carrying a known action open expanded straight away — the
  // button must be clickable without a hover dance (AC-008-05 gives the
  // user 15 s).
  return hovered || noticeActionFor(state.notice?.action) !== null
    ? "notice-expanded"
    : "notice";
}

/** FR-008-10: collapse the toast after this much no-interaction time. */
export const COLLAPSE_AFTER_MS = 60_000;
/** FR-008-09: how long the "Gravando · <App>" confirmation stays up. */
export const CONFIRMATION_MS = 3_000;

/** Actions sent to `detector_respond` (T-061 owns the command; camelCase
 *  args per the tauri-specta convention — `detectionId`). */
export type DetectorAction =
  | "start"
  | "start_mic_only"
  | "always"
  | "never"
  | "ignore_meeting"
  | "dismiss";

/** Actions that start recording — success shows "Gravando · <App>" for 3 s
 *  (FR-008-09); `always`/`never` also write the rule change server-side. */
export const RECORDING_ACTIONS: ReadonlySet<DetectorAction> = new Set([
  "start",
  "start_mic_only",
  "always",
]);

export function startsRecording(action: DetectorAction): boolean {
  return RECORDING_ACTIONS.has(action);
}

/** A `toast://show` action the toast can answer inline (T-069): the button
 *  an expanded notice renders. `command` is invoked raw — meeting commands
 *  ship in bindings.ts, but this webview deliberately calls `invoke` (same
 *  style as `detector_respond` above). */
export interface NoticeAction {
  command: string;
  /** Key under `toast.*` holding the button label. */
  labelKey: string;
}

/** Known notice actions, keyed by `toast://show.action`. Notices whose
 *  action is absent here render without a button — e.g. `open_consent`
 *  opens the Hub modal (MeetingConsentGate) and `checkin` needs the
 *  two-choice prompt (Continuar/Parar) that belongs to T-066's surface. */
const NOTICE_ACTIONS: Readonly<Record<string, NoticeAction>> = {
  // FR-008-14: "Continuar gravando" cancels the 15 s auto-stop.
  continue_recording: {
    command: "meeting_continue_recording",
    labelKey: "keepRecording",
  },
  // FR-009-08: "Estender 30 min" on the duration-limit warning.
  extend_30: { command: "meeting_extend_30", labelKey: "extend30" },
};

export function noticeActionFor(action: unknown): NoticeAction | null {
  return typeof action === "string" ? (NOTICE_ACTIONS[action] ?? null) : null;
}

export interface ToastMenuItem {
  action: DetectorAction;
  /** Key under `toast.*` in the translation files (takes `{{app}}`). */
  labelKey: string;
}

/** The ▾ menu, in spec order (FR-008-08). */
export function meetingMenuItems(): ToastMenuItem[] {
  return [
    { action: "start_mic_only", labelKey: "startMicOnly" },
    { action: "always", labelKey: "alwaysStartFor" },
    { action: "never", labelKey: "neverAskFor" },
    { action: "ignore_meeting", labelKey: "ignoreMeeting" },
  ];
}

/**
 * Window height per view in CSS px, including the 16 px stage padding top
 * and bottom — keep in sync with the fixed card heights in
 * `ToastOverlay.css`. Reported to the backend via `toast_set_content_height`
 * so the native window is only ever as tall as the card (no click-through
 * dead zone, and expansion grows upward from the anchored bottom edge).
 */
export function toastWindowHeight(view: ToastView, menuOpen: boolean): number {
  switch (view) {
    case "hidden":
      return 0;
    case "compact":
    case "confirming":
    case "notice":
      return 88; // 56 card + 32 stage padding
    case "notice-expanded":
      return 136; // 104 card + 32 padding (notices have no menu)
    case "expanded":
      return menuOpen ? 304 : 136; // 104 card (+168 menu) + 32 padding
  }
}
