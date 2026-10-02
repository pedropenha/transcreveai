import assert from "node:assert/strict";
import { appIconSlug, createAppIconResolver } from "./appIconCore";

// Label → slug: case-insensitive, aliases for the long names.
assert.equal(appIconSlug("Zoom"), "zoom");
assert.equal(appIconSlug("  ZOOM  "), "zoom");
assert.equal(appIconSlug("Google Meet"), "meet");
assert.equal(appIconSlug("google meet"), "meet");
assert.equal(appIconSlug("Meet"), "meet");
assert.equal(appIconSlug("Microsoft Teams"), "teams");
assert.equal(appIconSlug("Teams"), "teams");
assert.equal(appIconSlug("Webex"), "webex");
assert.equal(appIconSlug("Cisco Webex"), "webex");
assert.equal(appIconSlug("Discord"), "discord");
assert.equal(appIconSlug("Slack"), "slack");
assert.equal(appIconSlug("Unknown App"), null);
// Whole-word matching: substrings of other words must not match.
assert.equal(appIconSlug("Meeting Recorder"), null);
assert.equal(appIconSlug("Steamsong"), null);
assert.equal(appIconSlug("Meetup"), null);
assert.equal(appIconSlug("Microsoft Teams (work)"), "teams");
assert.equal(appIconSlug("Zoom Workplace"), "zoom");
assert.equal(appIconSlug(""), null);
assert.equal(appIconSlug("   "), null);

const resolve = createAppIconResolver({ zoom: "/assets/zoom.svg" });

// A backend-provided PNG data URI wins over the bundled logo.
assert.deepEqual(resolve("Zoom", "data:image/png;base64,AAAA"), {
  kind: "img",
  src: "data:image/png;base64,AAAA",
});
// Anything that is not a PNG data URI is ignored (no remote/script URLs).
assert.deepEqual(resolve("Zoom", "https://evil.example/x.png"), {
  kind: "img",
  src: "/assets/zoom.svg",
});
assert.deepEqual(resolve("Zoom", "data:text/html;base64,AAAA"), {
  kind: "img",
  src: "/assets/zoom.svg",
});
assert.deepEqual(resolve("Zoom", null), {
  kind: "img",
  src: "/assets/zoom.svg",
});
assert.deepEqual(resolve("Zoom"), { kind: "img", src: "/assets/zoom.svg" });
// Known slug without a bundled file (phase C not landed) → fallback.
assert.deepEqual(resolve("Slack", null), { kind: "fallback" });
assert.deepEqual(resolve("Unknown", null), { kind: "fallback" });
assert.deepEqual(resolve("", undefined), { kind: "fallback" });
// Empty icon string is treated as absent.
assert.deepEqual(resolve("Zoom", ""), { kind: "img", src: "/assets/zoom.svg" });
console.log("appIconCore tests passed");
