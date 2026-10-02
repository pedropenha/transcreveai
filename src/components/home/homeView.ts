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
