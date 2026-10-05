import assert from "node:assert/strict";
import {
  canDragPanel,
  errorKindKey,
  isPanelDragging,
  pinToggleKey,
  providerHintIsAdvisory,
  providerHintKey,
  safeMarkdownUrl,
} from "./assistantView";

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

// --- providerHintIsAdvisory: only the experimental flag doesn't block ---
// An explicitly selected experimental provider is usable (the user opted
// in) — every other hint explains why the provider can't answer.
assert.equal(providerHintIsAdvisory("cli_agent_experimental"), true);
assert.equal(providerHintIsAdvisory("no_provider"), false);
assert.equal(providerHintIsAdvisory("cli_agent_not_detected"), false);
assert.equal(providerHintIsAdvisory("offline"), false);
assert.equal(providerHintIsAdvisory(null), false);
assert.equal(providerHintIsAdvisory(undefined), false);

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
assert.equal(canDragPanel(true), true);
assert.equal(pinToggleKey(false), "assistant.pin");
assert.equal(pinToggleKey(true), "assistant.unpin");
// The pointer-move handler only fires while a grab is active.
assert.equal(isPanelDragging(null), false);
assert.equal(isPanelDragging({ clientX: 10, clientY: 8 }), true);

console.log("assistantView: all assertions passed");
