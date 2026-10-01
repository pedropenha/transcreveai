import assert from "node:assert/strict";
import {
  CONFIRMATION_MS,
  COLLAPSE_AFTER_MS,
  meetingMenuItems,
  noticeActionFor,
  noticeBlocksCollapse,
  resolveToastView,
  startsRecording,
  toastWindowHeight,
  type MeetingDetection,
  type ToastNotice,
  type ToastStateEvent,
  type ToastViewInput,
} from "./toastView";

const detection: MeetingDetection = {
  detection_id: "d1",
  app_label: "Google Meet",
  exe: "chrome.exe",
  action: "ask",
  started_at: 123,
};

const notice: ToastNotice = { kind: "warning", message: "mic fallback" };

const idle: ToastStateEvent = {
  collapsed: false,
  detection: null,
  notice: null,
};
const meeting: ToastStateEvent = { collapsed: false, detection, notice: null };

const base: ToastViewInput = {
  state: meeting,
  hovered: false,
  menuOpen: false,
  confirming: null,
};

// Nothing to show → hidden; collapsed → hidden (the amber dot lives on the
// Flow Bar, FR-008-10/12).
assert.equal(resolveToastView({ ...base, state: idle }), "hidden");
assert.equal(
  resolveToastView({ ...base, state: { ...meeting, collapsed: true } }),
  "hidden",
);
assert.equal(
  resolveToastView({
    ...base,
    state: { collapsed: true, detection: null, notice },
  }),
  "hidden",
);

// Compact face (FR-008-07) and hover expansion (FR-008-08).
assert.equal(resolveToastView(base), "compact");
assert.equal(resolveToastView({ ...base, hovered: true }), "expanded");
assert.equal(resolveToastView({ ...base, menuOpen: true }), "expanded");

// Confirmation outranks expansion and survives hover loss (FR-008-09).
assert.equal(
  resolveToastView({ ...base, confirming: "Google Meet" }),
  "confirming",
);
assert.equal(
  resolveToastView({ ...base, hovered: true, confirming: "Google Meet" }),
  "confirming",
);
// A cleared detection drops the confirmation instead of ghosting it.
assert.equal(
  resolveToastView({ ...base, state: idle, confirming: "Google Meet" }),
  "hidden",
);

// Generic toast://show notices: compact, expand on hover.
const noticed: ToastViewInput = {
  ...base,
  state: { collapsed: false, detection: null, notice },
};
assert.equal(resolveToastView(noticed), "notice");
assert.equal(
  resolveToastView({ ...noticed, hovered: true }),
  "notice-expanded",
);

// T-069 (AC-008-05): notices carrying a known action open expanded without
// hover — the "Continuar gravando" button must be reachable inside 15 s.
const autoStopNotice: ToastNotice = {
  kind: "meeting_auto_stop",
  message: "A reunião terminou — finalizando a gravação em 15 s…",
  action: "continue_recording",
};
assert.equal(
  resolveToastView({
    ...base,
    state: { collapsed: false, detection: null, notice: autoStopNotice },
  }),
  "notice-expanded",
);
const limitNotice: ToastNotice = {
  kind: "meeting_limit",
  message: "limite em 5 min",
  action: "extend_30",
};
assert.equal(
  resolveToastView({
    ...base,
    state: { collapsed: false, detection: null, notice: limitNotice },
  }),
  "notice-expanded",
);
// FR-009-09/AC-009-08: the silence check-in opens expanded — Continuar /
// Parar must be clickable immediately — and is exempt from the 60 s
// collapse so it survives its 2-minute response window.
const checkinNotice: ToastNotice = {
  kind: "meeting_checkin",
  message: "Ainda em reunião?",
  action: "checkin",
};
assert.equal(
  resolveToastView({
    ...base,
    state: { collapsed: false, detection: null, notice: checkinNotice },
  }),
  "notice-expanded",
);
assert.equal(noticeBlocksCollapse(checkinNotice), true);
assert.equal(noticeBlocksCollapse(notice), false);
const checkin = noticeActionFor("checkin");
assert.equal(checkin?.command, "meeting_checkin_respond");
assert.deepEqual(checkin?.args, { keepRecording: true });
assert.equal(checkin?.secondary?.command, "meeting_checkin_respond");
assert.deepEqual(checkin?.secondary?.args, { keepRecording: false });

// FR-009-22: retry_processing needs the notice's meeting_id — without it
// the action is unanswerable and the notice stays compact.
const retryNotice: ToastNotice = {
  kind: "meeting_error",
  message: "falha ao processar",
  action: "retry_processing",
};
assert.equal(noticeActionFor("retry_processing"), null);
assert.equal(noticeActionFor("retry_processing", null), null);
const retry = noticeActionFor("retry_processing", "m-1");
assert.equal(retry?.command, "meeting_retry_processing");
assert.equal(retry?.needsMeetingId, true);
assert.equal(
  resolveToastView({
    ...base,
    state: {
      collapsed: false,
      detection: null,
      notice: { ...retryNotice, meeting_id: "m-1" },
    },
  }),
  "notice-expanded",
);
assert.equal(
  resolveToastView({
    ...base,
    state: { collapsed: false, detection: null, notice: retryNotice },
  }),
  "notice",
);

// open_summary_settings shows the Hub and deep-links Settings → General.
const summaryNotice = noticeActionFor("open_summary_settings");
assert.equal(summaryNotice?.command, "show_main_window_command");
assert.equal(summaryNotice?.navigateSection, "settings");
assert.equal(summaryNotice?.navigateSettingsTab, "general");

// Actions the toast cannot answer inline keep the compact notice face.
assert.equal(
  resolveToastView({
    ...base,
    state: {
      collapsed: false,
      detection: null,
      notice: { kind: "meeting_consent", message: "x", action: "open_consent" },
    },
  }),
  "notice",
);
assert.equal(
  noticeActionFor("continue_recording")?.command,
  "meeting_continue_recording",
);
assert.equal(noticeActionFor("extend_30")?.command, "meeting_extend_30");
assert.equal(noticeActionFor("bogus"), null);
assert.equal(noticeActionFor(undefined), null);
// T-069: an auto-start detection never renders the ask prompt — the
// overlay feeds `confirming = app_label` into the resolver on the very
// first paint, so it lands on the "Gravando · <App>" face directly.
assert.equal(
  resolveToastView({
    ...base,
    state: {
      collapsed: false,
      detection: { ...detection, action: "auto_start" },
      notice: null,
    },
    confirming: "Google Meet",
  }),
  "confirming",
);

// Which actions end in the "Gravando · <App>" confirmation (FR-008-09).
assert.equal(startsRecording("start"), true);
assert.equal(startsRecording("start_mic_only"), true);
assert.equal(startsRecording("always"), true);
assert.equal(startsRecording("never"), false);
assert.equal(startsRecording("ignore_meeting"), false);
assert.equal(startsRecording("dismiss"), false);

// The ▾ menu, in spec order (FR-008-08).
assert.deepEqual(
  meetingMenuItems().map((i) => i.action),
  ["start_mic_only", "always", "never", "ignore_meeting"],
);

// Timings per spec.
assert.equal(COLLAPSE_AFTER_MS, 60_000);
assert.equal(CONFIRMATION_MS, 3_000);

// Window heights track the view; hidden reports 0 (never requested anyway).
assert.equal(toastWindowHeight("hidden", false), 0);
assert.equal(toastWindowHeight("compact", false), 88);
assert.equal(toastWindowHeight("confirming", false), 88);
assert.equal(toastWindowHeight("notice", false), 88);
assert.equal(toastWindowHeight("expanded", false), 136);
assert.equal(toastWindowHeight("expanded", true), 304);
assert.equal(toastWindowHeight("notice-expanded", false), 136);
assert.equal(toastWindowHeight("notice-expanded", true), 136);

console.log("toastView tests passed");
