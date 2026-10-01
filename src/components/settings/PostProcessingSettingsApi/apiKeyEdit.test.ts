import assert from "node:assert/strict";
import { apiKeyEditValue } from "./apiKeyEdit";

const HINT = "••••1234";

// Regression: backspacing inside the mask must never submit mask fragments
// like "123" as if they were a new API key.
assert.equal(apiKeyEditValue(HINT, "••••123"), HINT);
assert.equal(apiKeyEditValue(HINT, "••••123x"), HINT);
assert.equal(apiKeyEditValue(HINT, "•••1234"), HINT);
assert.equal(apiKeyEditValue(HINT, HINT), HINT);

// Select-all-then-type produces bullet-free text — accepted as a new key.
assert.equal(apiKeyEditValue(HINT, "sk-live-new-key"), "sk-live-new-key");

// Clearing the field is accepted (means "remove the key" downstream).
assert.equal(apiKeyEditValue(HINT, ""), "");

// No hint displayed: normal typing flows through untouched.
assert.equal(apiKeyEditValue("", "sk-typed"), "sk-typed");
assert.equal(apiKeyEditValue("", ""), "");

// A pasted value containing a bullet is rejected too — real keys never
// contain one.
assert.equal(apiKeyEditValue(HINT, "sk-•-bogus"), HINT);

console.log("apiKeyEdit: all assertions passed");
