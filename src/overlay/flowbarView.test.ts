import assert from "node:assert/strict";
import {
  effectiveEdge,
  resolveFlowbarView,
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

// effectiveEdge mirrors geometry::effective_edge.
assert.equal(effectiveEdge("bottom", "bottom"), "bottom");
assert.equal(effectiveEdge("bottom", "top"), "top");
assert.equal(effectiveEdge("left", "top"), "left");
assert.equal(effectiveEdge("right", "bottom"), "right");
assert.equal(effectiveEdge(undefined, undefined), "bottom");

console.log("flowbarView tests passed");
