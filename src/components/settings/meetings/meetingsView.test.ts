import assert from "node:assert/strict";
import {
  formatMeetingDuration,
  meetingAppLabel,
  meetingDurationSeconds,
  normalizeMeetingStatus,
  statusBadgeVariant,
} from "./meetingsView";

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

// status normalization + badge variants
assert.equal(normalizeMeetingStatus("recording"), "recording");
assert.equal(normalizeMeetingStatus("bogus"), "ready");
assert.equal(statusBadgeVariant("recording"), "warning");
assert.equal(statusBadgeVariant("paused"), "secondary");
assert.equal(statusBadgeVariant("processing"), "primary");
assert.equal(statusBadgeVariant("ready"), "success");
assert.equal(statusBadgeVariant("error"), "danger");
assert.equal(statusBadgeVariant("recovered"), "secondary");

// app label fallback chain
assert.equal(
  meetingAppLabel({ app_label: "Google Meet", app_exe: "chrome.exe" }),
  "Google Meet",
);
assert.equal(
  meetingAppLabel({ app_label: null, app_exe: "zoom.exe" }),
  "zoom.exe",
);
assert.equal(meetingAppLabel({ app_label: "  ", app_exe: null }), "—");
assert.equal(meetingAppLabel({ app_label: null, app_exe: null }), "—");

console.log("meetingsView: all assertions passed");
