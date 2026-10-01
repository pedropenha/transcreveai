import assert from "node:assert/strict";
import {
  formatElapsedMs,
  formatSegmentTimestamp,
  mergeSegments,
  normalizeMeetingStatus,
  normalizeProgressStep,
  resolveSummaryPhase,
  segmentSpeaker,
  type MeetingSegmentLike,
} from "./meetingView";

const seg = (over: Partial<MeetingSegmentLike>): MeetingSegmentLike => ({
  id: "s-1",
  meeting_id: "m-1",
  track: "mic",
  speaker: null,
  start_ms: 0,
  end_ms: 1000,
  text: "texto",
  kind: "speech",
  is_final: true,
  ...over,
});

// --- Header timer -----------------------------------------------------------
assert.equal(formatElapsedMs(0), "0:00");
assert.equal(formatElapsedMs(59_999), "0:59");
assert.equal(formatElapsedMs(60_000), "1:00");
assert.equal(formatElapsedMs(599_500), "9:59");
assert.equal(formatElapsedMs(3_600_000), "1:00:00");
assert.equal(formatElapsedMs(3_661_400), "1:01:01");
assert.equal(formatElapsedMs(-5), "0:00");

// --- Transcript gutter ------------------------------------------------------
assert.equal(formatSegmentTimestamp(0), "[00:00]");
assert.equal(formatSegmentTimestamp(83_000), "[01:23]");
assert.equal(formatSegmentTimestamp(3_900_000), "[65:00]"); // minutes keep counting
assert.equal(formatSegmentTimestamp(-100), "[00:00]");

// --- Speaker labels (track + kind + optional diarized name) ------------------
assert.deepEqual(segmentSpeaker(seg({ track: "mic" })), { kind: "you" });
assert.deepEqual(segmentSpeaker(seg({ track: "system" })), {
  kind: "others",
});
assert.deepEqual(segmentSpeaker(seg({ track: "webrtc" })), { kind: "others" });
assert.deepEqual(segmentSpeaker(seg({ speaker: "Falante 1" })), {
  kind: "named",
  label: "Falante 1",
});
assert.deepEqual(segmentSpeaker(seg({ track: "system", speaker: "  Ana  " })), {
  kind: "named",
  label: "Ana",
});
// Markers win over speaker/track.
assert.deepEqual(segmentSpeaker(seg({ kind: "dictation_marker" })), {
  kind: "dictation",
});
assert.deepEqual(segmentSpeaker(seg({ kind: "gap_marker", speaker: "Você" })), {
  kind: "gap",
});

// --- Segment merge (hydration + live events) ---------------------------------
const hydrated = [
  seg({ id: "a", start_ms: 0 }),
  seg({ id: "b", start_ms: 5_000 }),
];
// Live event appends and the partial → final flip replaces by id.
const merged = mergeSegments(hydrated, [
  seg({ id: "b", start_ms: 5_000, text: "final", is_final: true }),
  seg({ id: "c", start_ms: 2_000 }),
]);
assert.deepEqual(
  merged.map((s) => s.id),
  ["a", "c", "b"],
);
assert.equal(merged[2].text, "final");
// Empty incoming is a no-op; unknown ids dedupe to one row.
assert.equal(mergeSegments(hydrated, []).length, 2);
assert.equal(
  mergeSegments(hydrated, [seg({ id: "a" }), seg({ id: "a" })]).length,
  2,
);

// --- Status mapping ----------------------------------------------------------
assert.equal(normalizeMeetingStatus("recording"), "recording");
assert.equal(normalizeMeetingStatus("paused"), "paused");
assert.equal(normalizeMeetingStatus("processing"), "processing");
assert.equal(normalizeMeetingStatus("ready"), "ready");
assert.equal(normalizeMeetingStatus("error"), "error");
assert.equal(normalizeMeetingStatus("recovered"), "recovered");
assert.equal(normalizeMeetingStatus("whatever-t067-adds"), null);

// --- Summary phase -----------------------------------------------------------
const base = {
  status: "recording",
  summaryMd: null,
  summaryStatus: null,
  llmConfigured: true,
  progressActive: false,
};
assert.equal(resolveSummaryPhase(base), "pending");
assert.equal(resolveSummaryPhase({ ...base, status: "paused" }), "pending");
// FR-009-21: no key → disabled even while the meeting is live.
assert.equal(
  resolveSummaryPhase({ ...base, llmConfigured: false }),
  "disabled",
);
assert.equal(
  resolveSummaryPhase({
    ...base,
    llmConfigured: false,
    status: "ready",
  }),
  "disabled",
);
assert.equal(
  resolveSummaryPhase({ ...base, status: "processing" }),
  "generating",
);
assert.equal(
  resolveSummaryPhase({ ...base, progressActive: true }),
  "generating",
);
assert.equal(
  resolveSummaryPhase({ ...base, status: "ready", summaryMd: "# Resumo" }),
  "ready",
);
assert.equal(resolveSummaryPhase({ ...base, status: "error" }), "error");
// Ended without a summary but with a key configured → retryable failure.
assert.equal(resolveSummaryPhase({ ...base, status: "ready" }), "error");
// T-067's own vocabulary takes precedence when it lands.
assert.equal(
  resolveSummaryPhase({ ...base, summaryStatus: "generating" }),
  "generating",
);
assert.equal(
  resolveSummaryPhase({
    ...base,
    summaryStatus: "disabled",
    llmConfigured: true,
  }),
  "disabled",
);
assert.equal(
  resolveSummaryPhase({
    ...base,
    summaryMd: "x",
    summaryStatus: "ready",
    llmConfigured: false,
  }),
  "ready",
);

// --- Progress step ------------------------------------------------------------
assert.equal(normalizeProgressStep("transcribe"), "transcribe");
assert.equal(normalizeProgressStep("  summarize "), "summarize");
assert.equal(normalizeProgressStep(""), null);
assert.equal(normalizeProgressStep(undefined), null);

console.log("meetingView tests passed");
