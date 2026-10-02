import assert from "node:assert/strict";
import type { MeetingListItem, MeetingSegment } from "@/bindings";
import {
  applyProgress,
  countProcessing,
  detectionStatus,
  formatClock,
  groupMeetingsByDay,
  iconCacheKey,
  itemsForTab,
  logoCandidates,
  monogram,
  pruneProgress,
  retryActionFor,
  rowChip,
  splitMeetings,
  summarySections,
  transcriptExcerpt,
} from "./notetakerView";

const at = (y: number, m: number, d: number, h = 12, min = 0) =>
  Math.floor(new Date(y, m - 1, d, h, min).getTime() / 1000);

const item = (
  id: string,
  startedAt: number,
  listStatus: MeetingListItem["list_status"],
  status = "ready",
): MeetingListItem =>
  ({
    id,
    title: id,
    status,
    started_at: startedAt,
    ended_at: startedAt + 60,
    list_status: listStatus,
    source_app: null,
  }) as unknown as MeetingListItem;

// --- row chip ---------------------------------------------------------------
assert.deepEqual(rowChip("ready"), {
  tone: "success",
  labelKey: "notetaker.status.ready",
  pct: null,
});
assert.equal(rowChip("processing", 64.4).pct, 64);
assert.equal(rowChip("processing", 180).pct, 100);
assert.equal(rowChip("processing", -3).pct, 0);
assert.equal(rowChip("processing").pct, null);
assert.equal(rowChip("no_summary").tone, "warning");
assert.equal(rowChip("failed").tone, "error");
assert.equal(rowChip("paused").labelKey, "notetaker.status.paused");
assert.equal(rowChip("ready", 50).pct, null, "pct only while processing");

// --- retry routing ---------------------------------------------------------
assert.equal(
  retryActionFor({ list_status: "failed", status: "error" }),
  "retry_processing",
);
assert.equal(
  retryActionFor({ list_status: "failed", status: "recovered" }),
  "retry_processing",
);
assert.equal(
  retryActionFor({ list_status: "failed", status: "ready" }),
  "regenerate_summary",
);
assert.equal(retryActionFor({ list_status: "ready", status: "ready" }), null);
assert.equal(
  retryActionFor({ list_status: "no_summary", status: "ready" }),
  null,
);

// --- split / tabs / grouping ---------------------------------------------------
const list = [
  item("old", at(2026, 9, 29, 14), "ready"),
  item("live", at(2026, 10, 2, 11), "recording", "recording"),
  item("proc", at(2026, 10, 2, 8), "processing", "processing"),
  item("yday", at(2026, 10, 1, 19), "failed", "error"),
];
const { live, past } = splitMeetings(list);
assert.equal(live?.id, "live");
assert.deepEqual(
  past.map((m) => m.id),
  ["proc", "yday", "old"],
  "past is newest first and excludes the live one",
);
assert.equal(list[0]?.id, "old", "the input is not mutated");
assert.equal(splitMeetings([]).live, null);
assert.equal(splitMeetings([item("p", 1, "paused", "paused")]).live?.id, "p");
assert.deepEqual(
  itemsForTab(past, "processing").map((m) => m.id),
  ["proc"],
);
assert.equal(itemsForTab(past, "past").length, 3);
assert.equal(countProcessing(past), 1);

const groups = groupMeetingsByDay(past, new Date(2026, 9, 2, 15, 30));
assert.deepEqual(
  groups.map((g) => g.kind),
  ["today", "yesterday", "date"],
);
assert.deepEqual(
  groups.map((g) => g.entries.map((m) => m.id)),
  [["proc"], ["yday"], ["old"]],
);

// --- logo + monogram ---------------------------------------------------------
assert.deepEqual(logoCandidates(null), []);
assert.deepEqual(logoCandidates({ exe: "chrome.exe", name: "Google Meet" }), [
  "Google Meet",
  "chrome",
]);
assert.deepEqual(logoCandidates({ exe: null, name: "Zoom" }), ["Zoom"]);
assert.equal(iconCacheKey({ exe: " Zoom.EXE ", name: "Zoom" }), "zoom.exe");
assert.equal(iconCacheKey({ exe: null, name: "Zoom" }), null);
assert.equal(iconCacheKey(null), null);
assert.equal(monogram("obs studio").letter, "O");
assert.equal(monogram("").letter, "?");
assert.equal(monogram(undefined).letter, "?");
assert.equal(monogram("Foo").hue, monogram("foo").hue, "hue ignores case");
assert.ok(monogram("Foo").hue >= 0 && monogram("Foo").hue < 360);

// --- clock -------------------------------------------------------------------
assert.equal(formatClock(0), "00:00");
assert.equal(formatClock(724), "12:04");
assert.equal(formatClock(3725), "1:02:05");
assert.equal(formatClock(-5), "00:00");

// --- summary sections --------------------------------------------------------
assert.deepEqual(summarySections(null), []);
assert.deepEqual(summarySections("   "), []);
assert.deepEqual(summarySections("Just a paragraph."), [
  { heading: null, body: "Just a paragraph." },
]);
assert.deepEqual(
  summarySections(
    "Intro\r\n\r\n## Decisões\r\n- A\r\n- B\r\n\r\n### Próximos passos ##\r\n- [ ] C\r\n## Vazia\r\n",
  ),
  [
    { heading: null, body: "Intro" },
    { heading: "Decisões", body: "- A\n- B" },
    { heading: "Próximos passos", body: "- [ ] C" },
  ],
);

// --- transcript excerpt -------------------------------------------------------
const seg = (
  id: string,
  startMs: number,
  text: string,
  over: Partial<MeetingSegment> = {},
): MeetingSegment => ({
  id,
  meeting_id: "m",
  track: "system",
  speaker: "Outros",
  start_ms: startMs,
  end_ms: startMs + 1000,
  text,
  kind: "speech",
  is_final: true,
  excluded: false,
  ...over,
});
const excerpt = transcriptExcerpt(
  [
    seg("a", 12_000, " Bom dia "),
    seg("b", 20_000, "marcador", { kind: "dictation_marker" }),
    seg("c", 25_000, "parcial", { is_final: false }),
    seg("d", 30_000, "excluído", { excluded: true }),
    seg("e", 31_000, "   "),
    seg("f", 107_000, "Segundo"),
    seg("g", 120_000, "Terceiro"),
  ],
  2,
);
assert.deepEqual(excerpt, [
  { id: "a", time: "00:12", speaker: "Outros", text: "Bom dia" },
  { id: "f", time: "01:47", speaker: "Outros", text: "Segundo" },
]);

// --- progress ----------------------------------------------------------------
const empty = new Map<string, number>();
const withOne = applyProgress(empty, { meeting_id: "m1", pct: 64 });
assert.equal(withOne.get("m1"), 64);
assert.equal(empty.size, 0, "applyProgress never mutates");
assert.equal(applyProgress(withOne, { meeting_id: "m1" }), withOne);
assert.equal(applyProgress(withOne, { meeting_id: "m1", pct: NaN }), withOne);
assert.equal(
  applyProgress(withOne, { meeting_id: "m1", pct: 130 }).get("m1"),
  100,
);
const pruned = pruneProgress(withOne, [item("m1", 1, "ready")]);
assert.equal(pruned.size, 0);
assert.equal(
  pruneProgress(withOne, [item("m1", 1, "processing", "processing")]),
  withOne,
  "unchanged map is returned as is",
);

// --- detection -----------------------------------------------------------------
const base = { enabled: true, pausedUntilMs: null, offline: false };
assert.equal(detectionStatus(base, 1000), "active");
assert.equal(detectionStatus({ ...base, enabled: false }, 1000), "off");
assert.equal(detectionStatus({ ...base, offline: true }, 1000), "off");
assert.equal(detectionStatus({ ...base, pausedUntilMs: 5000 }, 1000), "paused");
assert.equal(detectionStatus({ ...base, pausedUntilMs: 500 }, 1000), "active");
assert.equal(
  detectionStatus({ ...base, pausedUntilMs: 1000 }, 1000),
  "active",
  "the pause ends exactly at its deadline",
);
assert.deepEqual(groupMeetingsByDay([], new Date()), []);
console.log("notetakerView: all assertions passed");
