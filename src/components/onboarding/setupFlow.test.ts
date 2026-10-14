import assert from "node:assert/strict";
import type { CliAgentStatus, ModelInfo } from "@/bindings";
import {
  agentToAdopt,
  detectedAgents,
  hitKeys,
  pickSetupModel,
  providerReady,
  shortcutKeys,
  shortcutMatched,
  showAgentBanner,
} from "./setupFlow";

const model = (id: string, overrides: Partial<ModelInfo> = {}): ModelInfo => ({
  id,
  name: id,
  description: "",
  filename: `${id}.gguf`,
  source: "Local",
  size_mb: 740,
  is_downloaded: false,
  is_downloading: false,
  partial_size: 0,
  is_directory: false,
  engine_type: "TranscribeCpp",
  accuracy_score: 0,
  speed_score: 0,
  supports_translation: false,
  is_recommended: false,
  supported_languages: [],
  supports_language_selection: false,
  is_custom: false,
  supports_streaming: false,
  supports_language_detection: false,
  ...overrides,
});

const agent = (
  provider_id: string,
  overrides: Partial<CliAgentStatus> = {},
): CliAgentStatus =>
  ({
    provider_id,
    label: provider_id,
    binary: provider_id.split("/")[1],
    detected: true,
    enabled: true,
    binary_name: null,
    experimental: false,
    install_hint: "",
    ...overrides,
  }) as CliAgentStatus;

// --- pickSetupModel -------------------------------------------------------

const gguf = model("handy-computer/parakeet-tdt-0.6b-v3-gguf/x-Q8_0.gguf");
const legacy = model("parakeet-tdt-0.6b-v3", {
  source: { Url: { url: "https://x", sha256: null } } as ModelInfo["source"],
});
const englishOnly = model(
  "handy-computer/parakeet-unified-en-0.6b-gguf/x.gguf",
  {
    is_recommended: true,
  },
);

// Only the multilingual catalog GGUF — never the legacy blob or English-only.
assert.equal(pickSetupModel([legacy, englishOnly, gguf])?.id, gguf.id);
assert.equal(pickSetupModel([legacy, englishOnly]), undefined);
// An already-downloaded copy wins over a fresh download.
const onDisk = model("handy-computer/parakeet-tdt-0.6b-v3-gguf/x-Q4.gguf", {
  is_downloaded: true,
});
assert.equal(pickSetupModel([gguf, onDisk])?.id, onDisk.id);

// --- detectedAgents -------------------------------------------------------

assert.deepEqual(
  detectedAgents([
    agent("cli_agent/codex"),
    agent("cli_agent/claude"),
    agent("cli_agent/cursor_agent", { experimental: true }),
  ]),
  ["cli_agent/claude", "cli_agent/codex"],
);
assert.deepEqual(
  detectedAgents([
    agent("cli_agent/claude", { detected: false }),
    agent("cli_agent/codex", { enabled: false }),
  ]),
  [],
);

// --- providerReady --------------------------------------------------------

const claudeOnly = [
  agent("cli_agent/claude"),
  agent("cli_agent/codex", { detected: false }),
];
assert.equal(providerReady("openai", {}, claudeOnly), false);
assert.equal(providerReady("openai", { openai: "••••1234" }, claudeOnly), true);
assert.equal(providerReady("custom", {}, claudeOnly), true);
assert.equal(providerReady("cli_agent/claude", {}, claudeOnly), true);
assert.equal(providerReady("cli_agent/codex", {}, claudeOnly), false);
assert.equal(providerReady(undefined, {}, claudeOnly), false);

// --- agentToAdopt ---------------------------------------------------------

// Fresh install: default openai without a key → switch to the detected agent.
assert.equal(agentToAdopt(null, "openai", {}, claudeOnly), "cli_agent/claude");
// A BYOK provider that works is never overwritten without an explicit pick…
assert.equal(agentToAdopt(null, "openai", { openai: "k" }, claudeOnly), null);
// …but an explicit pick on the setup screen wins.
const both = [agent("cli_agent/claude"), agent("cli_agent/codex")];
assert.equal(
  agentToAdopt("cli_agent/codex", "cli_agent/claude", {}, both),
  "cli_agent/codex",
);
// Already on the chosen agent → nothing to do.
assert.equal(agentToAdopt(null, "cli_agent/claude", {}, both), null);
// Nothing installed → nothing to adopt.
assert.equal(agentToAdopt(null, "openai", {}, []), null);
// A pick that is not installed falls back to the preferred detected agent.
assert.equal(
  agentToAdopt("cli_agent/codex", "openai", {}, claudeOnly),
  "cli_agent/claude",
);

// --- shortcut check -------------------------------------------------------

assert.deepEqual(shortcutKeys("ctrl+space"), ["ctrl", "space"]);
assert.deepEqual(shortcutKeys(" Ctrl + Shift + D "), ["ctrl", "shift", "d"]);
assert.deepEqual(shortcutKeys(undefined), []);

const keys = shortcutKeys("ctrl+space");
assert.equal(shortcutMatched(keys, new Set(["ctrl"])), false);
assert.equal(shortcutMatched(keys, new Set(["ctrl", "space"])), true);
assert.equal(shortcutMatched(keys, new Set(["ctrl", "space", "shift"])), true);
assert.equal(shortcutMatched([], new Set(["ctrl"])), false);
assert.deepEqual([...hitKeys(keys, new Set(["ctrl", "a"]))], ["ctrl"]);

// --- Notetaker banner -----------------------------------------------------

const none = [
  agent("cli_agent/claude", { detected: false }),
  agent("cli_agent/codex", { detected: false }),
];
const banner = {
  deferred: true,
  dismissed: false,
  providerId: "openai",
  apiKeyHints: {},
  statuses: none,
};
assert.equal(showAgentBanner(banner), true);
assert.equal(showAgentBanner({ ...banner, deferred: false }), false);
assert.equal(showAgentBanner({ ...banner, dismissed: true }), false);
assert.equal(showAgentBanner({ ...banner, statuses: null }), false);
assert.equal(showAgentBanner({ ...banner, statuses: claudeOnly }), false);
assert.equal(
  showAgentBanner({ ...banner, apiKeyHints: { openai: "••••1234" } }),
  false,
);

console.log("setupFlow tests passed");
