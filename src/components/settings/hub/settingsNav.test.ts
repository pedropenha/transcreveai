import assert from "node:assert/strict";
import {
  DEFAULT_SETTINGS_PAGE,
  SETTINGS_CATEGORIES,
  SETTINGS_PAGES,
  categoryOf,
  pagesOf,
  resolveSettingsPage,
} from "./settingsNav";

// Full paths resolve to themselves.
for (const page of SETTINGS_PAGES) {
  assert.equal(resolveSettingsPage(page), page);
}

// Every legacy flat tab keeps working (deep links from before the sub-nav).
assert.equal(resolveSettingsPage("general"), "usage/general");
assert.equal(resolveSettingsPage("models"), "transcription/models");
assert.equal(resolveSettingsPage("system"), "app/system");
assert.equal(resolveSettingsPage("privacy"), "app/privacy");
assert.equal(resolveSettingsPage("advanced"), "app/advanced");

// A bare category lands on its first page.
assert.equal(resolveSettingsPage("usage"), "usage/general");
assert.equal(resolveSettingsPage("transcription"), "transcription/models");
assert.equal(resolveSettingsPage("intelligence"), "intelligence/summaries");
assert.equal(resolveSettingsPage("app"), "app/system");

// Junk never resolves.
assert.equal(resolveSettingsPage("transcription/nope"), null);
assert.equal(resolveSettingsPage("../models"), null);
assert.equal(resolveSettingsPage(""), null);
assert.equal(resolveSettingsPage("   "), null);
assert.equal(resolveSettingsPage(null), null);
// Inherited object keys are not pages.
assert.equal(resolveSettingsPage("constructor"), null);
assert.equal(resolveSettingsPage("__proto__"), null);
assert.equal(resolveSettingsPage("toString"), null);
assert.equal(resolveSettingsPage(undefined), null);
assert.equal(resolveSettingsPage(" models "), "transcription/models");

// Structure: every page belongs to a known category and the default exists.
assert.ok(SETTINGS_PAGES.includes(DEFAULT_SETTINGS_PAGE));
for (const category of SETTINGS_CATEGORIES) {
  const pages = pagesOf(category);
  assert.ok(pages.length > 0, `${category} has pages`);
  assert.ok(pages.every((entry) => categoryOf(entry.id) === category));
}
assert.deepEqual(
  pagesOf("transcription").map((entry) => entry.slug),
  ["models", "languages", "api"],
);
assert.deepEqual(
  pagesOf("intelligence").map((entry) => entry.slug),
  ["summaries", "assistant"],
);

console.log("settingsNav: all assertions passed");
