// View-model helpers for the Hub "Reuniões" section (T-068, FR-009-25).
// Pure functions so the list rendering and status mapping stay unit-testable
// without React (same pattern as overlay/flowbarView.ts).

import type { Meeting } from "@/bindings";

export const SEARCH_DEBOUNCE_MS = 300;

export const MEETING_STATUSES = [
  "recording",
  "paused",
  "processing",
  "ready",
  "error",
  "recovered",
] as const;

export type MeetingStatus = (typeof MEETING_STATUSES)[number];

export type MeetingBadgeVariant =
  | "primary"
  | "success"
  | "warning"
  | "secondary"
  | "danger";

/// Fold unknown/legacy status strings onto the closest known state so the
/// UI never renders an unlocalized badge.
export const normalizeMeetingStatus = (status: string): MeetingStatus =>
  (MEETING_STATUSES as readonly string[]).includes(status)
    ? (status as MeetingStatus)
    : "ready";

export const statusBadgeVariant = (status: string): MeetingBadgeVariant => {
  switch (normalizeMeetingStatus(status)) {
    case "recording":
      // Amber — actively recording, matches the Flow Bar indicator tone.
      return "warning";
    case "paused":
      return "secondary";
    case "processing":
      return "primary";
    case "ready":
      return "success";
    case "error":
      return "danger";
    case "recovered":
      return "secondary";
  }
};

/// `ended_at - started_at` in seconds, or null while the meeting has not
/// ended (recording / paused) or when the timestamps are inconsistent.
export const meetingDurationSeconds = (
  meeting: Pick<Meeting, "started_at" | "ended_at">,
): number | null => {
  if (meeting.ended_at === null) return null;
  const seconds = meeting.ended_at - meeting.started_at;
  return seconds >= 0 ? seconds : null;
};

/// Same compact format the Markdown export uses: `"32 min"`,
/// `"1 h 12 min"`, `"—"` when there is no duration yet.
export const formatMeetingDuration = (seconds: number | null): string => {
  if (seconds === null) return "—";
  const totalMin = Math.floor(seconds / 60);
  const hours = Math.floor(totalMin / 60);
  const minutes = totalMin % 60;
  return hours > 0 ? `${hours} h ${minutes} min` : `${minutes} min`;
};

/// Display label for the app column: friendly label, then the exe name,
/// then the em-dash placeholder for manually started meetings.
export const meetingAppLabel = (
  meeting: Pick<Meeting, "app_label" | "app_exe">,
): string => meeting.app_label?.trim() || meeting.app_exe?.trim() || "—";
