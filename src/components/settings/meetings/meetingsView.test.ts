import assert from "node:assert/strict";
import { formatMeetingDuration, meetingDurationSeconds } from "./meetingsView";

// duration extraction
assert.equal(
  meetingDurationSeconds({ started_at: 1000, ended_at: 2935 }),
  1935,
);
assert.equal(
  meetingDurationSeconds({ started_at: 1000, ended_at: null }),
  null,
);
assert.equal(meetingDurationSeconds({ started_at: 1000, ended_at: 900 }), null);

// duration formatting — mirrors the Markdown exporter's format
assert.equal(formatMeetingDuration(null), "—");
assert.equal(formatMeetingDuration(0), "0 min");
assert.equal(formatMeetingDuration(59), "0 min");
assert.equal(formatMeetingDuration(60), "1 min");
assert.equal(formatMeetingDuration(1935), "32 min");
assert.equal(formatMeetingDuration(4320), "1 h 12 min");
assert.equal(formatMeetingDuration(3600), "1 h 0 min");

console.log("meetingsView: all assertions passed");
