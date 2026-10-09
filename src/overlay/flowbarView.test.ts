import assert from "node:assert/strict";
import {
  effectiveEdge,
  hoverTipParts,
  hoverTipText,
  meetingStateClaimsFlowbar,
  resolveFlowbarView,
  toastBadgeVisible,
  type FlowbarViewInput,
} from "./flowbarView";

const base: FlowbarViewInput = {
  alwaysOn: true,
  windowActive: true,
  phase: "idle",
  meeting: "idle",
  notice: null,
  hint: null,
  hovered: false,
  retrying: false,
  modelLoading: false,
};

// Resting slit (FR-001-01) vs hover card (FR-001-02).
assert.equal(resolveFlowbarView(base), "idle");
assert.equal(resolveFlowbarView({ ...base, hovered: true }), "hover");

// Session-only mode renders nothing between sessions.
assert.equal(
  resolveFlowbarView({ ...base, alwaysOn: false, windowActive: false }),
  "hidden",
);

// Session lifecycle → the five F001 states.
assert.equal(resolveFlowbarView({ ...base, phase: "recording" }), "recording");
assert.equal(resolveFlowbarView({ ...base, phase: "arming" }), "recording");
assert.equal(resolveFlowbarView({ ...base, phase: "transcribing" }), "working");
assert.equal(resolveFlowbarView({ ...base, phase: "inserting" }), "working");
assert.equal(resolveFlowbarView({ ...base, phase: "done" }), "done");
assert.equal(resolveFlowbarView({ ...base, phase: "error" }), "error");

// "Nada ouvido": the coordinator's one-shot notice or the legacy hint.
assert.equal(
  resolveFlowbarView({ ...base, notice: "nothing_heard" }),
  "nothing-heard",
);
assert.equal(
  resolveFlowbarView({ ...base, hint: "nothing-heard" }),
  "nothing-heard",
);

// The live transcription panel keeps winning while the streaming hint is up.
assert.equal(
  resolveFlowbarView({ ...base, phase: "recording", hint: "streaming" }),
  "streaming",
);
assert.equal(
  resolveFlowbarView({ ...base, phase: "processing", hint: "streaming" }),
  "streaming",
);

// show-overlay arriving before the coordinator's first event still renders
// the right face (event race hardening).
assert.equal(
  resolveFlowbarView({ ...base, phase: "idle", hint: "recording" }),
  "recording",
);
assert.equal(
  resolveFlowbarView({ ...base, phase: "idle", hint: "transcribing" }),
  "working",
);

// An in-flight retry shows the working face instead of the stale error.
assert.equal(
  resolveFlowbarView({ ...base, phase: "error", retrying: true }),
  "working",
);

// T-113: a model load in flight during a session claims the "carregando"
// face over capture and pipeline phases — the pill explains the pause.
// Terminal faces still win, and an inactive bar never flashes it.
assert.equal(
  resolveFlowbarView({ ...base, phase: "recording", modelLoading: true }),
  "loading-model",
);
assert.equal(
  resolveFlowbarView({ ...base, phase: "transcribing", modelLoading: true }),
  "loading-model",
);
assert.equal(
  resolveFlowbarView({ ...base, phase: "done", modelLoading: true }),
  "done",
);
assert.equal(
  resolveFlowbarView({ ...base, phase: "error", modelLoading: true }),
  "error",
);
assert.equal(
  resolveFlowbarView({
    ...base,
    phase: "recording",
    modelLoading: true,
    windowActive: false,
  }),
  "recording",
);

// The terminal dwell outranks hover: an error pill hovering over the slit
// stays an error until the coordinator returns to idle.
assert.equal(
  resolveFlowbarView({ ...base, phase: "error", hovered: true }),
  "error",
);

// FR-009-07: a live meeting claims the pill (over idle/hover and leftover
// dictation hints) but never impersonates the dictation pill while one is
// actually capturing — ■ there ends-and-inserts, not stops the meeting.
assert.equal(
  resolveFlowbarView({ ...base, meeting: "recording" }),
  "meeting-recording",
);
assert.equal(
  resolveFlowbarView({ ...base, meeting: "paused" }),
  "meeting-recording",
);
assert.equal(
  resolveFlowbarView({ ...base, meeting: "recording", hovered: true }),
  "meeting-recording",
);
assert.equal(
  resolveFlowbarView({
    ...base,
    meeting: "recording",
    phase: "recording",
  }),
  "recording",
);
assert.equal(
  resolveFlowbarView({ ...base, meeting: "recording", phase: "done" }),
  "done",
);
// Meeting post-processing is not an idle state: the pill must explain the
// asynchronous work, the successful handoff and failures instead of
// disappearing while the transcript is still being built.
assert.equal(resolveFlowbarView({ ...base, meeting: "processing" }), "working");
assert.equal(resolveFlowbarView({ ...base, meeting: "ready" }), "done");
assert.equal(resolveFlowbarView({ ...base, meeting: "error" }), "error");
assert.equal(resolveFlowbarView({ ...base, meeting: "recovered" }), "done");
// Dictation keeps precedence while it has its own live/terminal lifecycle.
assert.equal(
  resolveFlowbarView({ ...base, meeting: "processing", phase: "recording" }),
  "recording",
);
assert.equal(
  resolveFlowbarView({ ...base, meeting: "processing", phase: "transcribing" }),
  "working",
);
// A show-overlay hint that races ahead of `session://state` is still a
// dictation face: the meeting pill's ■ must not hijack a dictation stop.
assert.equal(
  resolveFlowbarView({ ...base, meeting: "recording", hint: "recording" }),
  "recording",
);
assert.equal(
  resolveFlowbarView({ ...base, meeting: "recording", hint: "streaming" }),
  "streaming",
);
// `show-overlay("meeting")` is only a native wake hint: the meeting status,
// not a synthetic dictation face, chooses the rendered pill.
assert.equal(
  resolveFlowbarView({ ...base, meeting: "processing", hint: "meeting" }),
  "working",
);
assert.equal(
  resolveFlowbarView({ ...base, meeting: "idle", hint: "meeting" }),
  "idle",
);
assert.equal(
  resolveFlowbarView({ ...base, meeting: "recording", retrying: true }),
  "working",
);
assert.equal(
  resolveFlowbarView({
    ...base,
    meeting: "recording",
    notice: "nothing_heard",
  }),
  "nothing-heard",
);
// Unknown future statuses are deliberately safe: they collapse to idle rather
// than leaving a stale terminal face on screen.
assert.equal(resolveFlowbarView({ ...base, meeting: "unexpected" }), "idle");

// A delayed terminal event from an old meeting cannot steal the active pill;
// after the terminal face, a newer lifecycle may take over the resting bar.
assert.equal(
  meetingStateClaimsFlowbar("recording", "meeting-1", "ready", "meeting-2"),
  false,
);
assert.equal(
  meetingStateClaimsFlowbar("ready", "meeting-1", "error", "meeting-2"),
  true,
);
assert.equal(
  meetingStateClaimsFlowbar("ready", "meeting-1", "recording", "meeting-2"),
  true,
);
// Same-meeting ordering mirrors the backend snapshot: retry may return to
// processing, but a stale recording/paused tick must not resurrect the pill.
assert.equal(
  meetingStateClaimsFlowbar(
    "processing",
    "meeting-1",
    "recording",
    "meeting-1",
  ),
  false,
);
assert.equal(
  meetingStateClaimsFlowbar("ready", "meeting-1", "paused", "meeting-1"),
  false,
);
assert.equal(
  meetingStateClaimsFlowbar("ready", "meeting-1", "processing", "meeting-1"),
  true,
);
assert.equal(
  meetingStateClaimsFlowbar("paused", "meeting-1", "recording", "meeting-1"),
  true,
);
// FR-008-10/12: the collapsed-toast amber dot only exists while the bar
// itself renders a face.
assert.equal(toastBadgeVisible("idle", true), true);
assert.equal(toastBadgeVisible("recording", true), true);
assert.equal(toastBadgeVisible("hover", true), true);
assert.equal(toastBadgeVisible("hidden", true), false);
assert.equal(toastBadgeVisible("idle", false), false);

// effectiveEdge mirrors geometry::effective_edge.
assert.equal(effectiveEdge("bottom", "bottom"), "bottom");
assert.equal(effectiveEdge("bottom", "top"), "top");
assert.equal(effectiveEdge("left", "top"), "left");
assert.equal(effectiveEdge("right", "bottom"), "right");
assert.equal(effectiveEdge(undefined, undefined), "bottom");

console.log("flowbarView tests passed");

// Hover tooltip: "Ditar" + bold shortcut (reference image), plain label when
// no shortcut is configured, and a flat aria-label for assistive tech.
assert.deepEqual(hoverTipParts("Ditar", "Win + Space"), {
  label: "Ditar",
  shortcut: "Win + Space",
});
assert.deepEqual(hoverTipParts("Ditar", ""), {
  label: "Ditar",
  shortcut: null,
});
assert.deepEqual(hoverTipParts("Ditar", "   "), {
  label: "Ditar",
  shortcut: null,
});
assert.equal(
  hoverTipText({ label: "Ditar", shortcut: "Win + Space" }),
  "Ditar Win + Space",
);
assert.equal(hoverTipText({ label: "Notas", shortcut: null }), "Notas");
