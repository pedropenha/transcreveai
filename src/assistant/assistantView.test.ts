import assert from "node:assert/strict";
import {
  appendDictated,
  canSend,
  composerValue,
  hotkeyIntent,
  providerHintKey,
} from "./assistantView";

const ready = { phase: "idle" as const, providerReady: true };

// --- appendDictated (FR-012-12): the transcription lands in the field ---
assert.equal(appendDictated("", "olá"), "olá");
assert.equal(appendDictated("resuma", "isto"), "resuma isto");
// Dictation never erases what the user already typed.
assert.equal(appendDictated("  ", "t"), "t");
assert.equal(appendDictated("x", "   "), "x");
// Trailing whitespace on the draft doesn't double the separator.
assert.equal(appendDictated("resuma  ", "isto"), "resuma isto");

// --- composerValue: live preview composes over the draft while dictating ---
assert.equal(composerValue("olá", "mundo", true), "olá mundo");
assert.equal(composerValue("olá", "mundo", false), "olá");
assert.equal(composerValue("olá", "", true), "olá");

// --- canSend (FR-012-13/AC-012-06): explicit send only, provider-gated ---
assert.equal(canSend("pergunta", ready), true);
assert.equal(canSend("   ", ready), false);
assert.equal(canSend("q", { phase: "thinking", providerReady: true }), false);
assert.equal(canSend("q", { phase: "idle", providerReady: false }), false);
assert.equal(canSend("q", { phase: "error", providerReady: true }), true);
assert.equal(canSend("q", { phase: "cancelled", providerReady: true }), true);

// --- hotkeyIntent (FR-012-10/13): second press sends a non-empty draft ---
assert.equal(hotkeyIntent("pergunta", ready), "send");
assert.equal(hotkeyIntent("", ready), "focus");
assert.equal(hotkeyIntent("   ", ready), "focus");
assert.equal(
  hotkeyIntent("q", { phase: "thinking", providerReady: true }),
  "ignore",
);
// No provider: a draft press falls back to focusing the input — the banner
// explains why nothing can be sent.
assert.equal(
  hotkeyIntent("q", { phase: "idle", providerReady: false }),
  "focus",
);

// --- providerHintKey: backend hint → localized string ---
assert.equal(providerHintKey("no_provider"), "assistant.hint.noProvider");
assert.equal(providerHintKey("offline"), "assistant.hint.offline");
assert.equal(
  providerHintKey("missing_api_key"),
  "assistant.hint.missingApiKey",
);
assert.equal(providerHintKey("missing_model"), "assistant.hint.missingModel");
assert.equal(
  providerHintKey("cli_agent_disabled"),
  "assistant.hint.cliAgentDisabled",
);
assert.equal(
  providerHintKey("cli_agent_not_detected"),
  "assistant.hint.cliAgentNotDetected",
);
assert.equal(providerHintKey(null), "assistant.hint.noProvider");
assert.equal(providerHintKey("bogus"), "assistant.hint.noProvider");

console.log("assistantView: all assertions passed");
