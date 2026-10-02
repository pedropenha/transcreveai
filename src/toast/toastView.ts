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
  /** App icon as a PNG data URI (`data:image/png;base64,…`) or null. */
  icon?: string | null;
}

/** A generic `toast://show` warning/notice. */
export interface ToastNotice {
  kind: string;
  message: string;
  action?: unknown;
  /** Meeting the action acts on — `retry_processing` invokes
   *  `meeting_retry_processing(meetingId)`, so notices that need it but
   *  lack one render no button. Absent for detection-level notices. */
  meeting_id?: string | null;
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
  | "expanded" // always: icon + "Reunião detectada" + Iniciar Notetaker ▾ (FR-008-07/08)
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
  const { state, hovered, confirming } = input;

  if (state.detection === null && state.notice === null) return "hidden";
  // Collapsed = the amber dot on the Flow Bar is the whole UI (FR-008-10/12).
  if (state.collapsed) return "hidden";
  // The confirmation outranks expansion: after "Iniciar Notetaker" lands the
  // toast morphs to "Gravando · <App>" regardless of where the cursor sits.
  if (confirming !== null && state.detection !== null) return "confirming";
  if (state.detection !== null) {
    return "expanded";
  }
  // Notices carrying an answerable action open expanded straight away —
  // the button must be clickable without a hover dance (AC-008-05 gives
  // the user 15 s; FR-009-09's check-in gets 2 min).
  return hovered ||
    noticeActionFor(state.notice?.action, state.notice?.meeting_id) !== null
    ? "notice-expanded"
    : "notice";
}

/** FR-008-10: collapse the toast after this much no-interaction time. */
export const COLLAPSE_AFTER_MS = 60_000;
/** FR-008-09: how long the "Gravando · <App>" confirmation stays up. */
export const CONFIRMATION_MS = 3_000;
/** Generic notices self-dismiss after this long (hover pauses the clock);
 *  `noticeBlocksCollapse` kinds (the 2-min check-in) are exempt. The card
 *  shows the remaining time as a shrinking bar. */
export const NOTICE_AUTO_DISMISS_MS = 30_000;

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

/** One button of a notice action: the raw-invoked command plus the args
 *  it carries. `needsMeetingId` buttons only exist when the notice ships
 *  a `meeting_id`. */
export interface NoticeActionButton {
  command: string;
  /** Key under `toast.*` holding the button label. */
  labelKey: string;
  /** Extra invoke args (e.g. `{ keepRecording: false }` for "Parar"). */
  args?: Record<string, unknown>;
}

/** A `toast://show` action the toast can answer inline (T-069): the
 *  button(s) an expanded notice renders. `command` is invoked raw
 *  because the name is data-driven — a `commands.*` call needs a
 *  statically known member, so dynamic dispatch stays on `invoke`. */
export interface NoticeAction extends NoticeActionButton {
  /** Second button for two-choice prompts — FR-009-09's
   *  "Continuar"/"Parar" check-in. */
  secondary?: NoticeActionButton;
  /** The command takes `{ meetingId }` from `notice.meeting_id`. */
  needsMeetingId?: boolean;
  /** Hub sidebar section to open after `command` runs (`hub://navigate`
   *  payload) — `open_summary_settings` lands on the Settings hub. */
  navigateSection?: string;
  /** Settings hub tab to land on when `navigateSection` is "settings"
   *  (see `pendingSettingsTab`). */
  navigateSettingsTab?: string;
  /** The command resolves to text to put on the clipboard — the FR-009-02
   *  reminder's "Copiar aviso" (`meeting_consent_copy` → writeText). */
  copiesTextToClipboard?: boolean;
}

/** Known notice actions, keyed by `toast://show.action`. Notices whose
 *  action is absent here render without a button — e.g. `open_consent`
 *  opens the Hub modal (MeetingConsentGate) and unknown vocabulary stays
 *  informational rather than rendering a dead button. */
const NOTICE_ACTIONS: Readonly<Record<string, NoticeAction>> = {
  // FR-008-14: "Continuar gravando" cancels the 15 s auto-stop.
  continue_recording: {
    command: "meeting_continue_recording",
    labelKey: "keepRecording",
  },
  // FR-009-08: "Estender 30 min" on the duration-limit warning.
  extend_30: { command: "meeting_extend_30", labelKey: "extend30" },
  // FR-009-09/AC-009-08: "Ainda em reunião?" — Continuar keeps recording,
  // Parar stops and processes. `meeting_checkin_respond` acts on the
  // active session, so no meeting id is needed.
  checkin: {
    command: "meeting_checkin_respond",
    labelKey: "checkinContinue",
    args: { keepRecording: true },
    secondary: {
      command: "meeting_checkin_respond",
      labelKey: "checkinStop",
      args: { keepRecording: false },
    },
  },
  // FR-009-22: fatal post-processing failure — re-runs the pipeline for
  // the meeting the notice names. No meeting id → no button (the command
  // cannot be invoked without one).
  retry_processing: {
    command: "meeting_retry_processing",
    labelKey: "retryProcessing",
    needsMeetingId: true,
  },
  // `summary_status = disabled` notice: opens the Hub on the Settings →
  // General tab, where the summary provider/key is configured.
  open_summary_settings: {
    command: "show_main_window_command",
    labelKey: "openSettings",
    navigateSection: "settings",
    navigateSettingsTab: "intelligence/summaries",
  },
  // FR-009-02: the every-start consent reminder carries the notice text —
  // "Copiar aviso" puts it on the clipboard to paste in the meeting chat.
  copy_consent: {
    command: "meeting_consent_copy",
    labelKey: "copyNotice",
    copiesTextToClipboard: true,
  },
};

export function noticeActionFor(
  action: unknown,
  meetingId?: string | null,
): NoticeAction | null {
  if (typeof action !== "string") return null;
  const noticeAction = NOTICE_ACTIONS[action] ?? null;
  if (noticeAction?.needsMeetingId && !meetingId) return null;
  return noticeAction;
}

/** FR-009-09: the silence check-in must stay on screen for its whole
 *  2-minute response window — the 60 s collapse timer (FR-008-10) does
 *  not apply to notices that await an answer. */
export function noticeBlocksCollapse(notice: ToastNotice | null): boolean {
  return notice?.action === "checkin";
}

/** Whether a live notice gets the 30 s countdown bar + auto-dismiss —
 *  everything except kinds that await an answer (check-in's 2-min window). */
export function noticeSelfDismisses(notice: ToastNotice | null): boolean {
  return notice !== null && !noticeBlocksCollapse(notice);
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
 * and bottom. Pre-mount fallback only — once the card renders, the webview
 * reports its measured height instead (a wrapped notice message can grow
 * past the static values). Reported to the backend via
 * `toast_set_content_height` so the native window is only ever as tall as
 * the card (no click-through dead zone, and expansion grows upward from
 * the anchored bottom edge).
 */
export function toastWindowHeight(view: ToastView, menuOpen: boolean): number {
  switch (view) {
    case "hidden":
      return 0;
    case "confirming":
    case "notice":
      return 88; // 56 card + 32 stage padding
    case "notice-expanded":
      return 136; // 104 card + 32 padding (notices have no menu)
    case "expanded":
      return menuOpen ? 264 : 96; // 64 card (+168 menu) + 32 padding
  }
}
