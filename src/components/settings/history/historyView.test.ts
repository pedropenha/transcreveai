import assert from "node:assert/strict";
import {
  buildWordDiff,
  groupHistoryEntries,
  moveHistorySelection,
  type HistoryEntryView,
} from "./historyView";

const entry = (
  id: number,
  timestamp: number,
  over: Partial<HistoryEntryView> = {},
): HistoryEntryView => ({
  id,
  timestamp,
  appName: "Transcreve.ai",
  appExe: null,
  mode: "dictation",
  status: "inserted",
  rawText: "texto bruto",
  finalText: "texto final",
  ...over,
});

const groups = groupHistoryEntries(
  [entry(1, 1_700_000_000), entry(2, 1_700_000_100), entry(3, 1_699_900_000)],
  "pt-BR",
);
assert.equal(groups.length, 2);
assert.deepEqual(
  groups.flatMap((group) => group.entries.map((item) => item.id)),
  [1, 2, 3],
);

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
