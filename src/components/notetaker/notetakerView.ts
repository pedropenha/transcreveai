/**
 * Pure view-model helpers for the Notetaker screen (F009 FR-009-25/28, F010):
 * row chip, retry routing, day grouping, app-logo candidates, summary
 * sections and the transcript excerpt. No React, no IPC: unit-tested in
 * notetakerView.test.ts.
 */

import type {
  MeetingListItem,
  MeetingListStatus,
  MeetingSegment,
  SourceApp,
} from "@/bindings";
import { groupEntriesByDay, type DayGroup } from "../home/homeView";

export type ChipTone = "success" | "accent" | "warning" | "error" | "neutral";

export interface RowChip {
  tone: ChipTone;
  /** i18n key under `notetaker.status`. */
  labelKey: string;
  /** Transcription progress 0-100 while `processing`, when known. */
  pct: number | null;
}

/** The single chip a list row shows ("Estados da linha" in the proposal). */
export function rowChip(
  status: MeetingListStatus,
  progressPct: number | null = null,
): RowChip {
  switch (status) {
    case "ready":
      return { tone: "success", labelKey: "notetaker.status.ready", pct: null };
    case "processing":
      return {
        tone: "accent",
        labelKey: "notetaker.status.processing",
        pct: progressPct === null ? null : clampPct(progressPct),
      };
    case "no_summary":
      return {
        tone: "warning",
        labelKey: "notetaker.status.noSummary",
        pct: null,
      };
    case "failed":
      return { tone: "error", labelKey: "notetaker.status.failed", pct: null };
    case "recording":
      return {
        tone: "neutral",
        labelKey: "notetaker.status.recording",
        pct: null,
      };
    case "paused":
      return {
        tone: "neutral",
        labelKey: "notetaker.status.paused",
        pct: null,
      };
  }
}

function clampPct(value: number): number {
  if (!Number.isFinite(value)) return 0;
  return Math.min(100, Math.max(0, Math.round(value)));
}

export type RetryAction = "retry_processing" | "regenerate_summary";

/**
 * Which command retries a `failed` row: a pipeline that errored / was recovered
 * after a crash re-runs the whole processing; a summary that failed on an
 * otherwise `ready` meeting only regenerates the summary.
 */
export function retryActionFor(
  item: Pick<MeetingListItem, "list_status" | "status">,
): RetryAction | null {
  if (item.list_status !== "failed") return null;
  return item.status === "error" || item.status === "recovered"
    ? "retry_processing"
    : "regenerate_summary";
}

export type NotetakerTab = "past" | "processing";

export interface NotetakerSplit {
  /** The meeting being recorded now (recording or paused), if any. */
  live: MeetingListItem | null;
  /** Everything else, newest first. */
  past: MeetingListItem[];
}

export function splitMeetings(
  items: readonly MeetingListItem[],
): NotetakerSplit {
  const live =
    items.find(
      (m) => m.list_status === "recording" || m.list_status === "paused",
    ) ?? null;
  const past = items
    .filter((m) => m !== live)
    .sort((a, b) => b.started_at - a.started_at);
  return { live, past };
}

export function itemsForTab(
  past: readonly MeetingListItem[],
  tab: NotetakerTab,
): MeetingListItem[] {
  return tab === "processing"
    ? past.filter((m) => m.list_status === "processing")
    : [...past];
}

export function countProcessing(past: readonly MeetingListItem[]): number {
  return past.filter((m) => m.list_status === "processing").length;
}

/** Past meetings grouped by local day, newest first. */
export function groupMeetingsByDay(
  items: readonly MeetingListItem[],
  now: Date,
): DayGroup<MeetingListItem>[] {
  const wrapped = items.map((item) => ({ timestamp: item.started_at, item }));
  return groupEntriesByDay(wrapped, now).map((group) => ({
    ...group,
    entries: group.entries.map((entry) => entry.item),
  }));
}

const EXE_SUFFIX = /\.exe$/i;

/**
 * Labels to try, in order, against the bundled-logo rules: the friendly name
 * ("Google Meet" in Chrome) is more telling than the executable.
 */
export function logoCandidates(app: SourceApp | null): string[] {
  if (!app) return [];
  const labels = [app.name, (app.exe ?? "").replace(EXE_SUFFIX, "")];
  return labels.map((label) => label.trim()).filter((label) => label !== "");
}

export interface Monogram {
  letter: string;
  /** 0-359, stable per name, drives the monogram background. */
  hue: number;
}

export function monogram(name: string | null | undefined): Monogram {
  const trimmed = (name ?? "").trim();
  const letter = trimmed === "" ? "?" : trimmed.charAt(0).toUpperCase();
  let hash = 0;
  for (const char of trimmed.toLowerCase()) {
    hash = (hash * 31 + char.charCodeAt(0)) % 360;
  }
  return { letter, hue: hash };
}

/** Cache key shared by every meeting of the same executable. */
export function iconCacheKey(app: SourceApp | null): string | null {
  const exe = app?.exe?.trim();
  return exe ? exe.toLowerCase() : null;
}

const pad = (value: number) => String(value).padStart(2, "0");

/** `mm:ss`, or `h:mm:ss` from one hour on. */
export function formatClock(totalSeconds: number): string {
  const seconds = Math.max(0, Math.floor(totalSeconds));
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const rest = seconds % 60;
  return hours > 0
    ? `${hours}:${pad(minutes)}:${pad(rest)}`
    : `${pad(minutes)}:${pad(rest)}`;
}

/** Position of a segment in the meeting, e.g. `01:47`. */
export function formatSegmentTime(startMs: number): string {
  return formatClock(startMs / 1000);
}

export interface SummarySection {
  /** Heading text, or null for the text before the first heading. */
  heading: string | null;
  body: string;
}

const HEADING = /^#{1,3}\s+(.*?)\s*#*\s*$/;

/** Split a markdown summary on its `#`/`##`/`###` headings. */
export function summarySections(markdown: string | null): SummarySection[] {
  if (markdown === null || markdown.trim() === "") return [];
  const sections: { heading: string | null; lines: string[] }[] = [
    { heading: null, lines: [] },
  ];
  for (const line of markdown.split(/\r?\n/)) {
    const match = HEADING.exec(line);
    if (match) {
      sections.push({ heading: match[1] ?? "", lines: [] });
    } else {
      sections[sections.length - 1]?.lines.push(line);
    }
  }
  return sections
    .map((s) => ({ heading: s.heading, body: s.lines.join("\n").trim() }))
    .filter((s) => s.body !== "");
}

export interface ExcerptLine {
  id: string;
  time: string;
  speaker: string | null;
  text: string;
}

/** First spoken, final, non-excluded lines of the transcript. */
export function transcriptExcerpt(
  segments: readonly MeetingSegment[],
  max: number,
): ExcerptLine[] {
  return segments
    .filter(
      (s) =>
        s.kind === "speech" &&
        s.is_final &&
        !s.excluded &&
        s.text.trim() !== "",
    )
    .slice(0, max)
    .map((s) => ({
      id: s.id,
      time: formatSegmentTime(s.start_ms),
      speaker: s.speaker,
      text: s.text.trim(),
    }));
}

export type ProgressMap = ReadonlyMap<string, number>;

/** `meeting://progress` payload → new map (the input is never mutated). */
export function applyProgress(
  current: ProgressMap,
  payload: { meeting_id: string; pct?: number },
): ProgressMap {
  if (typeof payload.pct !== "number" || !Number.isFinite(payload.pct)) {
    return current;
  }
  const next = new Map(current);
  next.set(payload.meeting_id, clampPct(payload.pct));
  return next;
}

/** Drop the progress of meetings that are no longer processing. */
export function pruneProgress(
  current: ProgressMap,
  items: readonly Pick<MeetingListItem, "id" | "list_status">[],
): ProgressMap {
  const processing = new Set(
    items.filter((m) => m.list_status === "processing").map((m) => m.id),
  );
  const keep = [...current].filter(([id]) => processing.has(id));
  return keep.length === current.size ? current : new Map(keep);
}

export type DetectionStatus = "off" | "paused" | "active";

export interface DetectionInput {
  enabled: boolean;
  pausedUntilMs: number | null;
  offline: boolean;
}

export function detectionStatus(
  input: DetectionInput,
  nowMs: number,
): DetectionStatus {
  if (!input.enabled || input.offline) return "off";
  if (input.pausedUntilMs !== null && input.pausedUntilMs > nowMs) {
    return "paused";
  }
  return "active";
}
