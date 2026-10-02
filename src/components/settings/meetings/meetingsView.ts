// View-model helpers for the Notetaker list (T-068, FR-009-25).
// Pure functions so the list rendering and status mapping stay unit-testable
// without React (same pattern as overlay/flowbarView.ts).

import type { Meeting } from "@/bindings";

export const SEARCH_DEBOUNCE_MS = 300;

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
