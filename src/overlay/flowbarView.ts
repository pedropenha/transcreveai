/**
 * Pure view model for the Flow Bar (F001 / T-040): which visual the bar
 * shows, given the coordinator's `session://state`, the legacy overlay
 * show/hide hints, and pointer hover. Framework-free so every transition can
 * be unit-tested without a DOM — this is the AC-001-01..03 / AC-001-07..08
 * evidence.
 *
 * State sources:
 * - `session://state` (coordinator, contracts.md §5): the authoritative
 *   session lifecycle — idle/arming/recording/transcribing/processing/
 *   inserting/done/error plus a one-shot `notice` ("nothing_heard").
 * - `show-overlay`/`hide-overlay` (utils.rs): window activity + legacy layout
 *   hints. Kept as a fallback so an overlay shown before the coordinator
 *   event lands still renders something sensible.
 * - Hover/click-through: `flowbar_set_hover` bounds + `flowbar://cursor`.
 */

/** States in `SessionStateEvent.state` (transcription_coordinator/machine.rs). */
export type SessionPhase =
  | "idle"
  | "arming"
  | "recording"
  | "transcribing"
  | "processing"
  | "inserting"
  | "done"
  | "error";

/** Payload of the legacy `show-overlay` event (utils.rs show_* calls). */
export type OverlayHint =
  | "idle"
  | "recording"
  | "streaming"
  | "transcribing"
  | "processing"
  | "inserting"
  | "meeting"
  | "nothing-heard";

/** The Flow Bar's visual classes (F001 table + existing live stream). */
export type FlowbarView =
  | "hidden" // window unmapped or awaiting first hint (session-only mode)
  | "idle" // 48x8 slit (FR-001-01)
  | "hover" // 88x36 card, two actions + tooltip (FR-001-02..04)
  | "recording" // ✕ | waveform | ■ (FR-001-05/06, AC-001-07)
  | "meeting-recording" // pause/resume | timer | ■ — the meeting pill (FR-009-07)
  | "streaming" // live transcription panel (pre-existing)
  | "working" // transcribing/processing/inserting dots (AC-001-08)
  | "done" // ✓ flash
  | "error" // ⚠ pill; hover shows cause + retry (AC-001-08)
  | "nothing-heard"; // "Nada ouvido" flash (FR-002-14)

/** `meeting://state.status` reduced to what the bar needs (T-064/T-069).
 * Live statuses claim the meeting pill; post-processing statuses claim the
 * terminal faces ("processing" → working, "ready"/"recovered" → done,
 * "error" → error). Anything else collapses into "idle". */
export type MeetingStatus =
  | "idle"
  | "recording"
  | "paused"
  | "processing"
  | "ready"
  | "error"
  | "recovered";

const MEETING_TERMINAL_STATUSES = new Set(["ready", "error", "recovered"]);
const MEETING_POSTPROCESS_STATUSES = new Set([
  "processing",
  "ready",
  "error",
  "recovered",
]);

/** Whether an event may claim the Flow Bar's single meeting face. Lifecycle
 * events are still broadcast to the Hub; this only protects the pill from an
 * older meeting's post-processing while another meeting is active. */
export function meetingStateClaimsFlowbar(
  currentStatus: MeetingStatus,
  currentMeetingId: string | null,
  incomingStatus: string,
  incomingMeetingId: string,
): boolean {
  if (currentMeetingId === incomingMeetingId) {
    if (MEETING_POSTPROCESS_STATUSES.has(incomingStatus)) return true;
    // A stale capture tick must not regress processing/terminal back to live.
    return !(
      currentStatus === "processing" ||
      MEETING_TERMINAL_STATUSES.has(currentStatus)
    );
  }
  if (currentMeetingId === null) return true;
  if (incomingStatus === "recording" || incomingStatus === "paused") {
    return true;
  }
  return (
    MEETING_TERMINAL_STATUSES.has(currentStatus) &&
    MEETING_POSTPROCESS_STATUSES.has(incomingStatus)
  );
}

export interface FlowbarViewInput {
  /** `flowbar_visibility === "always"` && overlay enabled — the slit lives
   * on screen between sessions (FR-001-10). */
  alwaysOn: boolean;
  /** A `show-overlay` is currently in effect (last hide not consumed). */
  windowActive: boolean;
  /** Latest coordinator `session://state`. */
  phase: SessionPhase;
  /** Latest `meeting://state` status (T-064), "idle" when no meeting runs.
   * Accepts unknown future statuses — they collapse to the idle face. */
  meeting: MeetingStatus | (string & {});
  /** Latest `session://state.notice` (e.g. "nothing_heard"). */
  notice: string | null;
  /** Latest `show-overlay` hint, `null` after `hide-overlay`. */
  hint: OverlayHint | null;
  /** The idle slit is hovered (or the hover card is engaged). */
  hovered: boolean;
  /** "Tentar novamente" is in flight — keep the working face up. */
  retrying: boolean;
}

export function resolveFlowbarView(input: FlowbarViewInput): FlowbarView {
  const {
    alwaysOn,
    windowActive,
    phase,
    meeting,
    notice,
    hint,
    hovered,
    retrying,
  } = input;

  // An in-flight retry outranks the stored error state so the user sees the
  // bar working instead of a stale ⚠ while the wav re-transcribes.
  if (retrying) return "working";

  // Terminal dwell: the coordinator holds `done` 600 ms / `error` 3 s.
  if (phase === "done") return "done";
  if (phase === "error") return "error";

  // "Nada ouvido" — either the coordinator's one-shot notice or the legacy
  // nothing-heard hint.
  if (notice === "nothing_heard" || hint === "nothing-heard") {
    return "nothing-heard";
  }

  // Live session phases. The streaming panel wins whenever the hint asks for
  // it — the coordinator reports the same phases for streaming models.
  if (phase === "arming" || phase === "recording") {
    return hint === "streaming" ? "streaming" : "recording";
  }

  if (
    phase === "transcribing" ||
    phase === "processing" ||
    phase === "inserting"
  ) {
    return hint === "streaming" ? "streaming" : "working";
  }

  // phase === "idle": fall back to whatever the window was last told to show.
  // Covers the race where `show-overlay` arrives before the coordinator's
  // first state event. This must outrank the meeting pill: that pill's ■
  // stops the meeting and would otherwise hijack a dictation stop gesture.
  if (windowActive && hint && hint !== "idle" && hint !== "meeting") {
    if (hint === "streaming") return "streaming";
    if (hint === "recording") return "recording";
    return "working"; // transcribing/processing/inserting hints
  }

  // FR-009-07: a recording/paused meeting claims the pill while the
  // dictation session is idle — its ■ stops the *meeting*, so it must not
  // impersonate the dictation pill (whose ■ ends-and-inserts). Any dictation
  // face wins during the overlap; the red tray icon still marks the meeting.
  // Post-processing statuses keep a terminal face up instead of collapsing
  // to idle while the transcript/summary is still being built (T-069); an
  // unknown status falls through to the resting faces rather than pinning a
  // stale face on screen.
  if (meeting !== "idle" && phase === "idle") {
    if (meeting === "processing") return "working";
    if (meeting === "ready" || meeting === "recovered") return "done";
    if (meeting === "error") return "error";
    if (meeting === "recording" || meeting === "paused") {
      return "meeting-recording";
    }
  }

  // Resting: the slit only lives on screen when the bar is always-on or a
  // show is in effect; otherwise the webview renders nothing.
  if (!alwaysOn && !windowActive) return "hidden";
  return hovered ? "hover" : "idle";
}

/** The CSS dock edge the stage should mirror the native dock onto. */
export type StageEdge = "top" | "bottom" | "left" | "right";

/** Merge `flowbar_position_edge` (auto/left/right) with the legacy
 * `overlay_position` top/bottom — mirrors `geometry::effective_edge`. */
export function effectiveEdge(
  flowbarEdge: string | undefined,
  overlayPosition: string | undefined,
): StageEdge {
  if (flowbarEdge === "left") return "left";
  if (flowbarEdge === "right") return "right";
  return overlayPosition === "top" ? "top" : "bottom";
}

/**
 * FR-008-10/12 (T-062): a collapsed or suppressed meeting toast surfaces on
 * the Flow Bar as a small amber dot that reopens the toast on hover. The dot
 * only exists while the bar itself renders a face — a hidden bar shows
 * nothing (the toast://state consumer still reopens via `toast_reopen`).
 */
export function toastBadgeVisible(
  view: FlowbarView,
  toastPending: boolean,
): boolean {
  return toastPending && view !== "hidden";
}
