/**
 * Pure helpers behind the Início screen (F010): day grouping, streak, saved
 * time and greeting. No React, no IPC — unit-tested in homeView.test.ts.
 */

export type DayKind = "today" | "yesterday" | "date";

export interface DayGroup<T> {
  /** Local calendar day, `YYYY-MM-DD`. */
  key: string;
  kind: DayKind;
  /** Local noon of that day (safe for locale formatting). */
  date: Date;
  entries: T[];
}

const pad = (value: number) => String(value).padStart(2, "0");

function localDayKey(date: Date): string {
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

/** Local calendar day of a unix timestamp in seconds. */
export function dayKey(timestampSeconds: number): string {
  return localDayKey(new Date(timestampSeconds * 1000));
}

function shiftDays(from: Date, days: number): Date {
  return new Date(from.getFullYear(), from.getMonth(), from.getDate() + days);
}

/**
 * Group entries (already sorted newest first) by local day, tagging each group
 * as today / yesterday / a calendar date relative to `now`.
 */
export function groupEntriesByDay<T extends { timestamp: number }>(
  entries: readonly T[],
  now: Date,
): DayGroup<T>[] {
  const todayKey = localDayKey(now);
  const yesterdayKey = localDayKey(shiftDays(now, -1));
  const groups = new Map<string, DayGroup<T>>();

  for (const entry of entries) {
    const moment = new Date(entry.timestamp * 1000);
    const key = localDayKey(moment);
    const existing = groups.get(key);
    if (existing) {
      existing.entries.push(entry);
      continue;
    }
    const kind: DayKind =
      key === todayKey ? "today" : key === yesterdayKey ? "yesterday" : "date";
    groups.set(key, {
      key,
      kind,
      date: new Date(
        moment.getFullYear(),
        moment.getMonth(),
        moment.getDate(),
        12,
      ),
      entries: [entry],
    });
  }
  return [...groups.values()];
}

/**
 * Consecutive local days with activity, ending today. A day without activity
 * yet today does not break a streak that reached yesterday.
 */
export function computeStreak(
  timestampsSeconds: readonly number[],
  now: Date,
): number {
  const days = new Set(timestampsSeconds.map(dayKey));
  let cursor = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  if (!days.has(localDayKey(cursor))) cursor = shiftDays(cursor, -1);

  let streak = 0;
  while (days.has(localDayKey(cursor))) {
    streak += 1;
    cursor = shiftDays(cursor, -1);
  }
  return streak;
}

/** Split a duration in seconds into whole hours and rounded-up minutes. */
export function savedTimeParts(seconds: number): {
  hours: number;
  minutes: number;
} {
  const totalMinutes = Math.ceil(Math.max(0, seconds) / 60);
  return { hours: Math.floor(totalMinutes / 60), minutes: totalMinutes % 60 };
}

export type GreetingPeriod = "morning" | "afternoon" | "evening";

export function greetingPeriod(hour: number): GreetingPeriod {
  if (hour >= 5 && hour < 12) return "morning";
  if (hour >= 12 && hour < 18) return "afternoon";
  return "evening";
}

/**
 * True when `computeStreak` cannot grow by loading older entries: the loaded
 * timestamps already contain an activity day outside the streak (so the streak
 * broke inside the data), or there is nothing at all.
 */
export function isStreakExact(
  timestampsSeconds: readonly number[],
  now: Date,
): boolean {
  const distinctDays = new Set(timestampsSeconds.map(dayKey)).size;
  return distinctDays > computeStreak(timestampsSeconds, now);
}

// --- multi-select delete (T-115) -------------------------------------------

/** A row's checkbox/click flips its id in the selection set (non-mutating). */
export function toggleSelected(
  selected: ReadonlySet<number>,
  id: number,
): Set<number> {
  const next = new Set(selected);
  if (next.has(id)) next.delete(id);
  else next.add(id);
  return next;
}

/** Ids of the *loaded* rows that are selected — deletion only ever touches
 * rows the list currently displays, never stale ids from an older filter. */
export function selectedLoadedIds(
  entryIds: readonly number[],
  selected: ReadonlySet<number>,
): number[] {
  return entryIds.filter((id) => selected.has(id));
}

export type SelectAllState = "none" | "some" | "all";

/** Header checkbox state over the loaded rows: none / indeterminate / all. */
export function selectAllState(
  entryIds: readonly number[],
  selected: ReadonlySet<number>,
): SelectAllState {
  const count = selectedLoadedIds(entryIds, selected).length;
  if (count === 0) return "none";
  return count === entryIds.length ? "all" : "some";
}

/** Clicking the header checkbox: fully selected → drop the loaded ids from
 * the set; otherwise add every loaded id. */
export function applySelectAll(
  entryIds: readonly number[],
  selected: ReadonlySet<number>,
): Set<number> {
  if (selectAllState(entryIds, selected) === "all") {
    const drop = new Set(entryIds);
    return new Set([...selected].filter((id) => !drop.has(id)));
  }
  return new Set([...selected, ...entryIds]);
}

// --- origin app of a dictation --------------------------------------------

/** The slice of a history entry that identifies where it was dictated. */
export interface OriginApp {
  app_name?: string | null;
  app_exe?: string | null;
}

const EXE_SUFFIX = /\.exe$/i;

/** Name shown for the origin app, or `null` for pre-origin (legacy) entries. */
export function originLabel(entry: OriginApp): string | null {
  return entry.app_name?.trim() || entry.app_exe?.trim() || null;
}

/** Each row may have a different stored path, including legacy rows without
 *  one. The backend shares extraction results by path; IPC results belong to
 *  this entry so a missing legacy icon cannot hide a newer one. */
export function originIconKey(
  entry: OriginApp & { id: number },
): string | null {
  const exe = entry.app_exe?.trim();
  return exe ? `${entry.id}:${exe.toLowerCase()}` : null;
}

/** Labels tried, in order, against the embedded-logo rules: the friendly name
 *  first, then the exe stem. */
export function originLogoCandidates(entry: OriginApp): string[] {
  return [entry.app_name ?? "", (entry.app_exe ?? "").replace(EXE_SUFFIX, "")]
    .map((label) => label.trim())
    .filter((label) => label !== "");
}
