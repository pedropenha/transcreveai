import assert from "node:assert/strict";
import {
  handleNavigatePayload,
  onSettingsTabNavigate,
  takePendingSettingsTab,
} from "./pendingSettingsTab";

// A valid tab arriving while no hub is mounted is stashed, then drained
// exactly once by the next mount.
handleNavigatePayload({ settingsTab: "advanced" });
assert.equal(takePendingSettingsTab(), "app/advanced");
assert.equal(takePendingSettingsTab(), null);

// New paths and the old flat spelling resolve to the same page.
handleNavigatePayload({ settingsTab: "transcription/models" });
assert.equal(takePendingSettingsTab(), "transcription/models");
handleNavigatePayload({ settingsTab: "models" });
assert.equal(takePendingSettingsTab(), "transcription/models");

// Unknown/absent tabs never stash.
handleNavigatePayload({ settingsTab: "not-a-tab" });
handleNavigatePayload({});
assert.equal(takePendingSettingsTab(), null);

// A mounted hub gets live events — nothing is stashed meanwhile.
{
  const seen: string[] = [];
  const off = onSettingsTabNavigate((tab) => seen.push(tab));
  handleNavigatePayload({ settingsTab: "privacy" });
  assert.deepEqual(seen, ["app/privacy"]);
  assert.equal(takePendingSettingsTab(), null);
  off();
}

// The flow the bug broke: navigate → stash (hub not mounted) → mount drains
// the stash → later navigations arrive live again.
handleNavigatePayload({ settingsTab: "advanced" });
assert.equal(takePendingSettingsTab(), "app/advanced");
{
  const seen: string[] = [];
  const off = onSettingsTabNavigate((tab) => seen.push(tab));
  handleNavigatePayload({ settingsTab: "system" });
  off();
  assert.deepEqual(seen, ["app/system"]);
}

console.log("pendingSettingsTab: all assertions passed");
