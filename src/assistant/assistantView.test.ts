import assert from "node:assert/strict";
import {
  appendDictated,
  canDragPanel,
  canSend,
  composerValue,
  errorKindKey,
  hotkeyIntent,
  isPanelDragging,
  pinToggleKey,
  providerHintKey,
  safeMarkdownUrl,
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
assert.equal(
  providerHintKey("cli_agent_experimental"),
  "assistant.hint.cliAgentExperimental",
);
assert.equal(providerHintKey(null), "assistant.hint.noProvider");
assert.equal(providerHintKey("bogus"), "assistant.hint.noProvider");

// --- errorKindKey: the panel localizes via the typed kind, not raw text ---
assert.equal(errorKindKey("network"), "assistant.error.network");
assert.equal(errorKindKey("timeout"), "assistant.error.timeout");
assert.equal(errorKindKey("auth"), "assistant.error.auth");
assert.equal(errorKindKey("rate_limited"), "assistant.error.rateLimited");
assert.equal(errorKindKey("unavailable"), "assistant.error.unavailable");
assert.equal(errorKindKey("missing_api_key"), "assistant.error.missingApiKey");
assert.equal(errorKindKey("offline"), "assistant.error.offline");
assert.equal(errorKindKey("unsupported"), "assistant.error.unsupported");
assert.equal(errorKindKey("provider"), "assistant.error.provider");
assert.equal(errorKindKey(null), "assistant.error.provider");
assert.equal(errorKindKey("bogus"), "assistant.error.provider");

// --- safeMarkdownUrl: only http/https/mailto survive the link transform ---
assert.equal(safeMarkdownUrl("https://example.com/x"), "https://example.com/x");
assert.equal(safeMarkdownUrl("http://example.com"), "http://example.com");
assert.equal(safeMarkdownUrl("mailto:a@b.c"), "mailto:a@b.c");
assert.equal(safeMarkdownUrl("javascript:alert(1)"), "");
assert.equal(safeMarkdownUrl("data:text/html;base64,AAAA"), "");
assert.equal(safeMarkdownUrl("file:///etc/passwd"), "");
assert.equal(safeMarkdownUrl("JaVaScRiPt:alert(1)"), "");
// Schemeless / relative / anchors carry no external load — left untouched.
assert.equal(safeMarkdownUrl("/relative/path"), "/relative/path");
assert.equal(safeMarkdownUrl("#anchor"), "#anchor");

// --- T-092 drag/pin (FR-012-16): Fixar locks the panel ------------------
assert.equal(canDragPanel(false), true);
assert.equal(canDragPanel(true), false);
assert.equal(pinToggleKey(false), "assistant.pin");
assert.equal(pinToggleKey(true), "assistant.unpin");
// The pointer-move handler only fires while a grab is active.
assert.equal(isPanelDragging(null), false);
assert.equal(isPanelDragging({ clientX: 10, clientY: 8 }), true);

console.log("assistantView: all assertions passed");
