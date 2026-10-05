import assert from "node:assert/strict";
import {
  computeStreak,
  dayKey,
  greetingPeriod,
  groupEntriesByDay,
  isStreakExact,
  originIconKey,
  originLabel,
  originLogoCandidates,
  savedTimeParts,
} from "./homeView";

// Local-time helpers so the tests do not depend on the machine's time zone.
const at = (y: number, m: number, d: number, h = 12, min = 0) =>
  Math.floor(new Date(y, m - 1, d, h, min).getTime() / 1000);
const now = new Date(2026, 9, 2, 15, 30); // 2026-10-02 15:30 local

// --- day grouping --------------------------------------------------------
const entries = [
  { id: 1, timestamp: at(2026, 10, 2, 14, 12) },
  { id: 2, timestamp: at(2026, 10, 2, 9, 0) },
  { id: 3, timestamp: at(2026, 10, 1, 18, 35) },
  { id: 4, timestamp: at(2026, 9, 28, 8, 0) },
  { id: 5, timestamp: at(2025, 12, 31, 23, 59) },
];
const groups = groupEntriesByDay(entries, now);
assert.deepEqual(
  groups.map((g) => g.kind),
  ["today", "yesterday", "date", "date"],
);
assert.deepEqual(
  groups.map((g) => g.entries.map((e) => e.id)),
  [[1, 2], [3], [4], [5]],
);
assert.equal(groups[0].key, "2026-10-02");
assert.equal(groups[3].key, "2025-12-31");
// Input order is preserved inside a group and the input is not mutated.
assert.equal(entries.length, 5);
assert.deepEqual(groupEntriesByDay([], now), []);

// Late-night entries stay on their local day (no UTC drift).
assert.equal(dayKey(at(2026, 10, 2, 23, 59)), "2026-10-02");
assert.equal(dayKey(at(2026, 10, 3, 0, 1)), "2026-10-03");

// --- streak --------------------------------------------------------------
const ts = (...days: Array<[number, number, number]>) =>
  days.map(([y, m, d]) => at(y, m, d));
assert.equal(computeStreak([], now), 0);
// today + yesterday + 2 days ago => 3
assert.equal(
  computeStreak(ts([2026, 10, 2], [2026, 10, 1], [2026, 9, 30]), now),
  3,
);
// Nothing yet today but yesterday counts: the streak is still alive.
assert.equal(computeStreak(ts([2026, 10, 1], [2026, 9, 30]), now), 2);
// A gap ends the streak.
assert.equal(computeStreak(ts([2026, 10, 2], [2026, 9, 30]), now), 1);
// Last activity two days ago => broken.
assert.equal(computeStreak(ts([2026, 9, 30]), now), 0);
// Duplicates on the same day count once; order does not matter.
assert.equal(
  computeStreak(
    [at(2026, 10, 1, 8), at(2026, 10, 2, 9), at(2026, 10, 2, 20)],
    now,
  ),
  2,
);
// Month boundary.
assert.equal(
  computeStreak(
    ts([2026, 10, 1], [2026, 9, 30], [2026, 9, 29]),
    new Date(2026, 9, 1, 10),
  ),
  3,
);

// --- streak exactness ------------------------------------------------------
// A gap inside the loaded data proves the streak cannot grow.
assert.equal(isStreakExact(ts([2026, 10, 2], [2026, 9, 30]), now), true);
// Activity reaching the oldest loaded day: older pages could extend it.
assert.equal(isStreakExact(ts([2026, 10, 2], [2026, 10, 1]), now), false);
// No data: nothing to extend.
assert.equal(isStreakExact([], now), false);
// Latest activity older than yesterday: streak is 0 and final.
assert.equal(isStreakExact(ts([2026, 9, 20]), now), true);

// --- saved time ----------------------------------------------------------
assert.deepEqual(savedTimeParts(0), { hours: 0, minutes: 0 });
assert.deepEqual(savedTimeParts(59), { hours: 0, minutes: 1 });
assert.deepEqual(savedTimeParts(98 * 60), { hours: 1, minutes: 38 });
assert.deepEqual(savedTimeParts(3600), { hours: 1, minutes: 0 });
assert.deepEqual(savedTimeParts(-5), { hours: 0, minutes: 0 });

// --- greeting ------------------------------------------------------------
assert.equal(greetingPeriod(7), "morning");
assert.equal(greetingPeriod(12), "afternoon");
assert.equal(greetingPeriod(17), "afternoon");
assert.equal(greetingPeriod(18), "evening");
assert.equal(greetingPeriod(2), "evening");

// --- origin app ----------------------------------------------------------
assert.equal(
  originLabel({ app_name: "Claude", app_exe: "Claude.exe" }),
  "Claude",
);
assert.equal(originLabel({ app_name: null, app_exe: "tool.exe" }), "tool.exe");
assert.equal(originLabel({ app_name: "  ", app_exe: null }), null);
assert.equal(originLabel({ app_name: "  ", app_exe: "tool.exe" }), "tool.exe");
assert.equal(originLabel({}), null);
assert.equal(originIconKey({ id: 1, app_exe: " Claude.EXE " }), "1:claude.exe");
assert.notEqual(
  originIconKey({ id: 1, app_exe: "Claude.exe" }),
  originIconKey({ id: 2, app_exe: "claude.exe" }),
  "a legacy row without a stored path must not suppress another entry's icon",
);
assert.equal(originIconKey({ id: 1, app_exe: null }), null);
assert.equal(originIconKey({ id: 1, app_exe: "" }), null);
assert.deepEqual(
  originLogoCandidates({ app_name: "Teams", app_exe: "ms-teams.exe" }),
  ["Teams", "ms-teams"],
);
assert.deepEqual(originLogoCandidates({ app_name: null, app_exe: null }), []);

console.log("homeView tests passed");
