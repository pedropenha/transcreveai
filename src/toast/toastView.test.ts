import assert from "node:assert/strict";
import {
  CONFIRMATION_MS,
  COLLAPSE_AFTER_MS,
  meetingMenuItems,
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

const idle: ToastStateEvent = { collapsed: false, detection: null, notice: null };
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
const noticed: ToastViewInput = { ...base, state: { collapsed: false, detection: null, notice } };
assert.equal(resolveToastView(noticed), "notice");
assert.equal(resolveToastView({ ...noticed, hovered: true }), "notice-expanded");

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
