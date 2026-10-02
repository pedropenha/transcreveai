import assert from "node:assert/strict";
import {
  RAIL_ENTRIES,
  resolveNavigation,
  sectionForShortcut,
} from "./navModel";

const key = (k: string, over: Partial<KeyboardEvent> = {}) => ({
  key: k,
  ctrlKey: true,
  metaKey: false,
  shiftKey: false,
  altKey: false,
  ...over,
});

// Ctrl+1..5 follow the visible rail order (available sections only).
assert.equal(sectionForShortcut(key("1")), "home");
assert.equal(sectionForShortcut(key("2")), "meetings");
assert.equal(sectionForShortcut(key("3")), "dictionary");
assert.equal(sectionForShortcut(key("4")), "settings");
assert.equal(sectionForShortcut(key("5")), "help");
assert.equal(sectionForShortcut(key(",")), "settings");
// Cmd works on macOS; other modifier combos and bare keys are ignored.
assert.equal(
  sectionForShortcut(key("1", { ctrlKey: false, metaKey: true })),
  "home",
);
assert.equal(sectionForShortcut(key("6")), null);
assert.equal(sectionForShortcut(key("1", { ctrlKey: false })), null);
assert.equal(sectionForShortcut(key("1", { shiftKey: true })), null);
assert.equal(sectionForShortcut(key("1", { altKey: true })), null);
assert.equal(sectionForShortcut(key("a")), null);

// Legacy deep link: the "models" rail section moved into Settings.
assert.deepEqual(resolveNavigation({ section: "models" }), {
  section: "settings",
  settingsTab: "transcription/models",
});
// An explicit tab on a legacy payload wins over the redirect default.
assert.deepEqual(
  resolveNavigation({ section: "models", settingsTab: "advanced" }),
  { section: "settings", settingsTab: "advanced" },
);
assert.deepEqual(resolveNavigation({ section: "meetings" }), {
  section: "meetings",
});
assert.deepEqual(
  resolveNavigation({ section: "settings", settingsTab: "general" }),
  {
    section: "settings",
    settingsTab: "general",
  },
);
// Unknown / absent sections do not navigate.
assert.deepEqual(resolveNavigation({ section: "nope" }), { section: null });
assert.deepEqual(resolveNavigation({}), { section: null });
// A tab-only payload keeps the current section (the hub listener handles it).
assert.deepEqual(resolveNavigation({ settingsTab: "privacy" }), {
  section: null,
  settingsTab: "privacy",
});

// Rail structure: groups per the proposal, v1.1 entries are not navigable,
// and "models" is no longer a rail entry.
const ids = RAIL_ENTRIES.map((e) => e.id);
assert.ok(!ids.includes("models"));
assert.deepEqual(
  RAIL_ENTRIES.filter((e) => e.availability === "soon").map((e) => e.id),
  ["notes", "assistant"],
);
assert.deepEqual(
  RAIL_ENTRIES.filter((e) => e.group === "footer").map((e) => e.id),
  ["settings", "help"],
);
for (const entry of RAIL_ENTRIES.filter((e) => e.availability === "soon")) {
  assert.equal(entry.section, undefined);
}

console.log("navModel tests passed");
