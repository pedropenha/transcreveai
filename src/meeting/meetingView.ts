/**
 * Pure view model for the meeting window (F009 / T-066): formatting,
 * transcript dedupe, status mapping and the summary-tab state machine.
 * Framework-free so every branch is unit-tested without a DOM — this is the
 * AC-009-02/FR-009-12..14 evidence alongside the component.
 *
 * The `MeetingSegment` shape mirrors `meeting_segments` (db/meetings.rs) and
 * the `meeting://segment` payload (contracts.md §5): `track` is
 * 'mic' | 'system', `kind` is 'speech' | 'dictation_marker' | 'gap_marker'
 * and `is_final` flips when a partial transcript settles (T-065).
 */

/** Minimum shape of a `meeting_segments` row / `meeting://segment` payload. */
export interface MeetingSegmentLike {
  id: string;
  meeting_id: string;
  /** 'mic' | 'system' */
  track: string;
  /** "Você", "Outros", "Falante 1"… — absent for plain mic/system speech. */
  speaker: string | null;
  /** Milliseconds relative to the meeting's `started_at`. */
  start_ms: number;
  end_ms: number;
  text: string;
  /** 'speech' | 'dictation_marker' | 'gap_marker' */
  kind: string;
  is_final: boolean;
  /** FR-009-10/AC-009-03: mic speech inside a dictation interval. Only ever
   * set on `kind === 'speech'` rows — markers never carry the flag. */
  excluded: boolean;
}

/** Statuses `meetings.status` can carry (db/meetings.rs CHECK constraint). */
export type MeetingStatus =
  | "recording"
  | "paused"
  | "processing"
  | "ready"
  | "error"
  | "recovered";

const KNOWN_STATUSES: readonly string[] = [
  "recording",
  "paused",
  "processing",
  "ready",
  "error",
  "recovered",
];

/** Defensive status read — an unexpected value renders as "error" styling
 * rather than crashing the window, but is kept distinguishable ("unknown"
 * maps onto the generic pill, not a misleading label). */
export function normalizeMeetingStatus(status: string): MeetingStatus | null {
  return (KNOWN_STATUSES as readonly string[]).includes(status)
    ? (status as MeetingStatus)
    : null;
}

/**
 * Header timer: `m:ss` under an hour, `h:mm:ss` above (matching the
 * Flow Bar's `fmtTime`, extended past the hour mark).
 */
export function formatElapsedMs(ms: number): string {
  const totalSeconds = Math.max(0, Math.floor(ms / 1000));
  const seconds = totalSeconds % 60;
  const minutes = Math.floor(totalSeconds / 60) % 60;
  const hours = Math.floor(totalSeconds / 3600);
  const ss = String(seconds).padStart(2, "0");
  if (hours > 0) {
    return `${hours}:${String(minutes).padStart(2, "0")}:${ss}`;
  }
  return `${minutes}:${ss}`;
}

/** Transcript gutter stamp: `[mm:ss]` relative to the meeting start
 * (FR-009-23 uses the same layout for "Copiar como Markdown"). */
export function formatSegmentTimestamp(startMs: number): string {
  const totalSeconds = Math.max(0, Math.floor(startMs / 1000));
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  return `[${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}]`;
}

/**
 * What the speaker column renders. Marker kinds win over the track: a
 * `gap_marker` row never claims a speaker, and a `dictation_marker` is the
 * user's own dictated note, not a participant.
 */
export type SegmentSpeaker =
  | { kind: "dictation" }
  | { kind: "gap" }
  | { kind: "named"; label: string }
  | { kind: "you" }
  | { kind: "others" };

export function segmentSpeaker(segment: MeetingSegmentLike): SegmentSpeaker {
  if (segment.kind === "dictation_marker") return { kind: "dictation" };
  if (segment.kind === "gap_marker") return { kind: "gap" };
  const named = segment.speaker?.trim();
  if (named) return { kind: "named", label: named };
  // T-065 fills `speaker` when diarization lands; until then the track is
  // the speaker: mic is the user, system loopback is everyone else.
  return segment.track === "mic" ? { kind: "you" } : { kind: "others" };
}

/**
 * FR-009-10/AC-009-03: `excluded` rows are dictated notes — private to the
 * user and never rendered as transcript speech. The `dictation_marker` row
 * covering the same span is what the transcript shows instead.
 */
export function isTranscriptVisible(segment: MeetingSegmentLike): boolean {
  return !segment.excluded;
}

/**
 * Merge hydrated rows with live `meeting://segment` payloads: dedupe by id
 * (the incoming copy wins — it may be the `is_final` flip of a partial),
 * then order by `start_ms` (stable — equal starts keep arrival order).
 */
export function mergeSegments<T extends MeetingSegmentLike>(
  existing: readonly T[],
  incoming: readonly MeetingSegmentLike[],
): T[] {
  if (incoming.length === 0) return [...existing];
  const byId = new Map<string, T>();
  for (const seg of existing) byId.set(seg.id, seg);
  for (const seg of incoming) byId.set(seg.id, seg as T);
  return [...byId.values()].sort((a, b) => a.start_ms - b.start_ms);
}

/**
 * "Resumo" tab state machine (FR-009-16..22). The inputs are deliberately
 * wide so T-067's fields plug in without a schema bump: `summary_status`
 * covers whatever dedicated column/event vocabulary lands there, while
 * `llmConfigured` (today: `meeting.llm_provider_id != null`) carries
 * FR-009-21's "no key configured" warning.
 */
export type SummaryPhase =
  | "pending" // still recording/paused — summary starts when the meeting stops
  | "disabled" // no LLM key (FR-009-21) — notes/transcript unaffected
  | "generating" // `meeting://progress` or `processing` status
  | "ready" // `summary_md` has content
  | "error"; // generation failed — "Tentar novamente" (FR-009-22)

export interface SummaryPhaseInput {
  /** `meetings.status`. */
  status: string;
  /** `meetings.summary_md`. */
  summaryMd: string | null;
  /** Defensive T-067 seam: a dedicated `summary_status` value when the
   * backend grows one ('pending'|'generating'|'ready'|'error'|'disabled'). */
  summaryStatus?: string | null;
  /** FR-009-21: an LLM provider is configured for this meeting. */
  llmConfigured: boolean;
  /** A `meeting://progress` event for this meeting arrived since the last
   * terminal state — shows the spinner even if `status` lags behind. */
  progressActive: boolean;
}

export function resolveSummaryPhase(input: SummaryPhaseInput): SummaryPhase {
  const { status, summaryMd, summaryStatus, llmConfigured, progressActive } =
    input;

  // Explicit T-067 vocabulary wins when present.
  switch (summaryStatus) {
    case "disabled":
    case "no_key":
      return "disabled";
    case "generating":
      return "generating";
    case "error":
      return "error";
    case "ready":
      return "ready";
    case "pending":
      return "pending";
    default:
      break;
  }

  if (summaryMd != null && summaryMd.trim() !== "") return "ready";

  // FR-009-21: without a configured LLM there is no pipeline to wait for.
  if (!llmConfigured) return "disabled";

  if (progressActive || status === "processing") return "generating";
  if (status === "error") return "error";

  // Meeting ended but no summary landed: with a key configured the pipeline
  // was supposed to produce one — surface as a retryable failure.
  if (status === "ready" || status === "recovered") return "error";

  return "pending"; // recording / paused / anything unknown
}

/** Progress label from `meeting://progress { step, pct }` — the step key is
 * passed through verbatim (T-067 owns the vocabulary); unknown/blank steps
 * render as a generic "processing" hint. */
export function normalizeProgressStep(step: string | undefined): string | null {
  const trimmed = step?.trim();
  return trimmed ? trimmed : null;
}
