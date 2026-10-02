import assert from "node:assert/strict";
import {
  SETUP_CHECKLIST_ID,
  buildSetupItems,
  isChecklistVisible,
  markSetupItemDone,
  setupProgress,
} from "./setupItems";

const base = { hasLocalModel: false, hasSummaryProvider: false, dismissed: [] };

let items = buildSetupItems(base);
assert.deepEqual(
  items.map((i) => i.id),
  ["model", "microphone", "system_audio", "summary_key"],
);
assert.ok(items.every((i) => !i.done));
assert.deepEqual(setupProgress(items), { done: 0, total: 4 });

// Derived items follow real state.
items = buildSetupItems({
  ...base,
  hasLocalModel: true,
  hasSummaryProvider: true,
});
assert.deepEqual(
  items.filter((i) => i.done).map((i) => i.id),
  ["model", "summary_key"],
);

// Manual items complete via a persisted id.
const afterMic = markSetupItemDone([], "microphone");
assert.deepEqual(afterMic, ["setup:microphone"]);
items = buildSetupItems({ ...base, dismissed: afterMic });
assert.equal(items.find((i) => i.id === "microphone")?.done, true);
// Immutable + idempotent.
const original: string[] = [];
assert.notEqual(markSetupItemDone(original, "microphone"), original);
assert.deepEqual(original, []);
assert.deepEqual(markSetupItemDone(afterMic, "microphone"), afterMic);

// Visibility: hidden once dismissed or when every item is done.
assert.equal(isChecklistVisible(items, []), true);
assert.equal(isChecklistVisible(items, [SETUP_CHECKLIST_ID]), false);
const all = buildSetupItems({
  hasLocalModel: true,
  hasSummaryProvider: true,
  dismissed: ["setup:microphone", "setup:system_audio"],
});
assert.deepEqual(setupProgress(all), { done: 4, total: 4 });
assert.equal(isChecklistVisible(all, []), false);

console.log("setupItems tests passed");
