import assert from "node:assert/strict";
import { buildWordDiff, moveHistorySelection } from "./historyView";

assert.equal(moveHistorySelection([1, 2, 3], null, "next"), 1);
assert.equal(moveHistorySelection([1, 2, 3], 1, "next"), 2);
assert.equal(moveHistorySelection([1, 2, 3], 3, "next"), 3);
assert.equal(moveHistorySelection([1, 2, 3], 2, "previous"), 1);
assert.equal(moveHistorySelection([], null, "next"), null);

assert.deepEqual(buildWordDiff("olá mundo", "olá novo mundo"), [
  { kind: "equal", text: "olá" },
  { kind: "added", text: "novo" },
  { kind: "equal", text: "mundo" },
]);
assert.deepEqual(buildWordDiff("remover isto agora", "remover agora"), [
  { kind: "equal", text: "remover" },
  { kind: "removed", text: "isto" },
  { kind: "equal", text: "agora" },
]);

console.log("historyView tests passed");
