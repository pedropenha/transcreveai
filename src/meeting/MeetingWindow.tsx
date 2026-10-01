import React, {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useTranslation } from "react-i18next";
import {
  FileText,
  LoaderCircle,
  MonitorPlay,
  NotebookPen,
  Pause,
  Pencil,
  Play,
  Sparkles,
  Square,
  TriangleAlert,
  User,
  Users,
} from "lucide-react";
import { commands } from "@/bindings";
import type {
  Meeting,
  MeetingDetail,
  MeetingSegment,
  MeetingStateEvent,
} from "@/bindings";
import i18n, { getLanguageDirection } from "@/i18n";
import { MarkdownContent } from "@/components/whats-new/MarkdownContent";
import {
  formatElapsedMs,
  formatSegmentTimestamp,
  isTranscriptVisible,
  mergeSegments,
  normalizeMeetingStatus,
  resolveSummaryPhase,
  segmentSpeaker,
  type MeetingSegmentLike,
} from "./meetingView";

// contracts.md §5 — `meeting://open` is emitted by meeting_window.rs each time
// `meeting_window_open` (re)targets this window at a meeting.
const MEETING_OPEN_EVENT = "meeting://open";
const MEETING_STATE_EVENT_NAME = "meeting://state";
const MEETING_SEGMENT_EVENT = "meeting://segment";
const MEETING_PROGRESS_EVENT = "meeting://progress";

const NOTES_AUTOSAVE_MS = 800;
const TITLE_MAX_CHARS = 160;

/** `meeting://open` payload (meeting_window.rs `MeetingOpenPayload`). */
interface MeetingOpenPayload {
  meeting_id: string;
}

/** `meeting://progress` payload (contracts.md §5, emitted by T-067). */
interface MeetingProgressPayload {
  meeting_id: string;
  step?: string;
  pct?: number;
}

type Tab = "notes" | "transcript" | "summary";

/** Fields T-067 may add to the meeting row — read defensively so the window
 * keeps working both before and after that schema lands. */
function readSummaryStatus(meeting: Meeting): string | null {
  const extra = meeting as unknown as {
    summary_status?: string | null;
    summaryStatus?: string | null;
  };
  return extra.summary_status ?? extra.summaryStatus ?? null;
}

const readMeetingIdFromUrl = (): string | null => {
  const id = new URLSearchParams(window.location.search).get("meeting_id");
  return id && id.trim() !== "" ? id : null;
};

const MeetingWindow: React.FC = () => {
  const { t } = useTranslation();
  const direction = getLanguageDirection(i18n.language);

  const [meetingId, setMeetingId] = useState<string | null>(
    readMeetingIdFromUrl,
  );
  const [meeting, setMeeting] = useState<Meeting | null>(null);
  const [segments, setSegments] = useState<MeetingSegment[]>([]);
  const [notesMd, setNotesMd] = useState("");
  const [notFound, setNotFound] = useState(false);
  // `null` meetingId before any resolution → still deciding, not "no meeting".
  const [resolved, setResolved] = useState(readMeetingIdFromUrl() !== null);

  // Live `meeting://state` for the shown meeting — status + ticking timer.
  const [liveStatus, setLiveStatus] = useState<string | null>(null);
  const [elapsedMs, setElapsedMs] = useState(0);

  // `meeting://progress` (T-067) — spinner on the summary tab. The step key
  // stays untranslated-by-design until T-067 ships its vocabulary, so the UI
  // only tracks that progress is active (never renders the raw step name).
  const [progressActive, setProgressActive] = useState(false);

  const [tab, setTab] = useState<Tab>("notes");
  const [editingTitle, setEditingTitle] = useState(false);
  const [titleDraft, setTitleDraft] = useState("");

  // Autosave bookkeeping — "Salvo às hh:mm" / "Salvando…" / failure hint.
  const [notesSaving, setNotesSaving] = useState(false);
  const [notesSavedAt, setNotesSavedAt] = useState<Date | null>(null);
  const [notesFailed, setNotesFailed] = useState(false);
  const [summarySaving, setSummarySaving] = useState(false);
  const [summarySavedAt, setSummarySavedAt] = useState<Date | null>(null);
  const [summaryEditing, setSummaryEditing] = useState(false);
  const [summaryDraft, setSummaryDraft] = useState<string | null>(null);

  const notesTimerRef = useRef<number | undefined>(undefined);
  const summaryTimerRef = useRef<number | undefined>(undefined);
  const transcriptEndRef = useRef<HTMLDivElement>(null);
  const pinnedRef = useRef(true);
  const transcriptScrollRef = useRef<HTMLDivElement>(null);
  const titleInputRef = useRef<HTMLInputElement>(null);

  // Latest notes text the backend knows about — lets the autosave skip a
  // no-op write when a rehydrate races an in-flight edit.
  const persistedNotesRef = useRef("");
  const notesMdRef = useRef("");
  useEffect(() => {
    notesMdRef.current = notesMd;
  }, [notesMd]);

  const loadMeeting = useCallback(async (id: string) => {
    const result = await commands.meetingGet(id);
    if (result.status !== "ok" || result.data === null) {
      setNotFound(true);
      setMeeting(null);
      return;
    }
    const detail: MeetingDetail = result.data;
    setNotFound(false);
    setMeeting(detail.meeting);
    // Keep any live `meeting://segment` rows that arrived while this
    // hydration was in flight — the snapshot may predate them.
    setSegments((prev) => mergeSegments(detail.segments, prev));
    // FR-009-19: a rehydrate (e.g. the `processing` state event on stop)
    // must never clobber in-flight edits — only adopt the stored body when
    // the editor is clean (local text == last persisted text).
    if (notesMdRef.current === persistedNotesRef.current) {
      persistedNotesRef.current = detail.notes_md;
      setNotesMd(detail.notes_md);
    }
    setNotesSavedAt(null);
    setNotesFailed(false);
    setLiveStatus(null);
    setElapsedMs(
      detail.meeting.ended_at != null
        ? Math.max(
            0,
            (detail.meeting.ended_at - detail.meeting.started_at) * 1000,
          )
        : 0,
    );
    setProgressActive(false);
    setSummaryEditing(false);
    setSummaryDraft(null);
  }, []);

  // Resolve which meeting to show: `?meeting_id=` wins, then the live session
  // (`meeting_current`), matching `meeting_window::resolve_meeting_id`.
  useEffect(() => {
    let cancelled = false;
    const resolve = async () => {
      const fromUrl = readMeetingIdFromUrl();
      if (fromUrl !== null) {
        setMeetingId(fromUrl);
        setResolved(true);
        return;
      }
      const current = await commands.meetingCurrent();
      if (cancelled) return;
      if (current.status === "ok" && current.data !== null) {
        setMeetingId(current.data.meeting_id);
      }
      setResolved(true);
    };
    void resolve();
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (meetingId !== null) void loadMeeting(meetingId);
  }, [meetingId, loadMeeting]);

  // Event listeners — all filtered to the shown meeting (the session events
  // are broadcast to every window).
  useEffect(() => {
    const unlisteners: Array<Promise<() => void>> = [];

    unlisteners.push(
      listen<MeetingOpenPayload>(MEETING_OPEN_EVENT, (event) => {
        const id = event.payload.meeting_id;
        if (id && id !== meetingId) setMeetingId(id);
      }),
    );

    unlisteners.push(
      listen<MeetingStateEvent>(MEETING_STATE_EVENT_NAME, (event) => {
        const payload = event.payload;
        if (payload.meeting_id !== meetingId) return;
        setLiveStatus(payload.status);
        setElapsedMs(payload.elapsed_ms);
        // A terminal transition lands out-of-band (T-067 writes the row):
        // re-hydrate so summary_md/status converge without a reload.
        if (payload.status !== "recording" && payload.status !== "paused") {
          void loadMeeting(payload.meeting_id);
        }
      }),
    );

    unlisteners.push(
      listen<MeetingSegment>(MEETING_SEGMENT_EVENT, (event) => {
        const seg = event.payload;
        if (seg.meeting_id !== meetingId) return;
        setSegments((prev) => mergeSegments(prev, [seg]));
      }),
    );

    unlisteners.push(
      listen<MeetingProgressPayload>(MEETING_PROGRESS_EVENT, (event) => {
        const payload = event.payload;
        if (payload.meeting_id !== meetingId) return;
        // `step` is accepted defensively (meetingView::normalizeProgressStep
        // owns the vocabulary seam) — the UI only tracks liveness until T-067
        // ships its step names.
        setProgressActive(true);
      }),
    );

    let cleanup: Array<() => void> = [];
    Promise.all(unlisteners).then((fns) => {
      cleanup = fns;
    });
    return () => {
      cleanup.forEach((fn) => fn());
    };
  }, [meetingId, loadMeeting]);

  // Autosave "Minhas notas" — debounced; writes only the note row
  // (FR-009-19: summary generation never touches this text).
  useEffect(() => {
    if (meetingId === null) return;
    if (notesMd === persistedNotesRef.current) return;
    setNotesSaving(true);
    setNotesFailed(false);
    window.clearTimeout(notesTimerRef.current);
    const body = notesMd;
    notesTimerRef.current = window.setTimeout(() => {
      void commands.meetingNotesUpdate(meetingId, body).then((result) => {
        if (result.status === "ok") {
          persistedNotesRef.current = body;
          setNotesSavedAt(new Date());
          setNotesSaving(false);
        } else {
          setNotesFailed(true);
          setNotesSaving(false);
        }
      });
    }, NOTES_AUTOSAVE_MS);
    return () => window.clearTimeout(notesTimerRef.current);
  }, [notesMd, meetingId]);

  // Autosave the editable summary (writes only `summary_md` — FR-009-19 cuts
  // both ways: editing the summary never touches the notes row).
  useEffect(() => {
    if (meetingId === null || summaryDraft === null) return;
    if (!summaryEditing) return;
    window.clearTimeout(summaryTimerRef.current);
    const body = summaryDraft;
    summaryTimerRef.current = window.setTimeout(() => {
      setSummarySaving(true);
      void commands.meetingSummaryUpdate(meetingId, body).then((result) => {
        setSummarySaving(false);
        if (result.status === "ok") {
          setSummarySavedAt(new Date());
          setMeeting((m) => (m ? { ...m, summary_md: body } : m));
        }
      });
    }, NOTES_AUTOSAVE_MS);
    return () => window.clearTimeout(summaryTimerRef.current);
  }, [summaryDraft, summaryEditing, meetingId]);

  // Focus the title input when entering rename mode.
  useEffect(() => {
    if (editingTitle) titleInputRef.current?.select();
  }, [editingTitle]);

  // Transcript auto-follow: keep pinned to the newest segment unless the user
  // scrolled up to read history.
  useEffect(() => {
    if (tab === "transcript" && pinnedRef.current) {
      transcriptEndRef.current?.scrollIntoView({ block: "end" });
    }
  }, [segments, tab]);

  const status = useMemo(() => {
    const raw = liveStatus ?? meeting?.status ?? "";
    return normalizeMeetingStatus(raw) ?? "error";
  }, [liveStatus, meeting]);

  // FR-009-10/AC-009-03: dictated mic rows (`excluded`) never render —
  // the `dictation_marker` row covering the span is shown instead.
  const visibleSegments = useMemo(
    () => segments.filter(isTranscriptVisible),
    [segments],
  );

  const live =
    meetingId !== null && (status === "recording" || status === "paused");

  const commitTitle = useCallback(async () => {
    if (meetingId === null || meeting === null) {
      setEditingTitle(false);
      return;
    }
    const title = titleDraft.trim().slice(0, TITLE_MAX_CHARS);
    setEditingTitle(false);
    if (title === "" || title === meeting.title) return;
    const result = await commands.meetingRename(meetingId, title);
    if (result.status === "ok") {
      setMeeting(result.data);
    } else {
      setTitleDraft(meeting.title);
    }
  }, [meetingId, meeting, titleDraft]);

  const togglePause = () => {
    void (status === "paused"
      ? commands.meetingResume()
      : commands.meetingPause());
  };
  const stopMeeting = () => {
    void commands.meetingStop();
  };

  // FR-009-20/22: "Tentar novamente" hits T-067's command when it exists —
  // `invoke` rejects with "unknown command" while it doesn't, and a local
  // re-check of the row keeps the UI honest either way.
  const [retryingSummary, setRetryingSummary] = useState(false);
  const retrySummary = async () => {
    if (meetingId === null) return;
    setRetryingSummary(true);
    try {
      await invoke("meeting_regenerate_summary", { meetingId });
      setProgressActive(true);
    } catch {
      // Command missing (pre-T-067 build) or refused — re-hydrate so the
      // phase falls back to whatever the row actually says.
      await loadMeeting(meetingId);
    } finally {
      setRetryingSummary(false);
    }
  };

  const speakerLabel = (seg: MeetingSegmentLike): string => {
    const speaker = segmentSpeaker(seg);
    switch (speaker.kind) {
      case "dictation":
        return t("meeting.window.dictationMarker");
      case "gap":
        return t("meeting.window.gapMarker");
      case "named":
        return speaker.label;
      case "you":
        return t("meeting.window.you");
      case "others":
        return t("meeting.window.others");
    }
  };

  const summaryPhase = resolveSummaryPhase({
    status,
    summaryMd: meeting?.summary_md ?? null,
    summaryStatus: meeting !== null ? readSummaryStatus(meeting) : null,
    llmConfigured: meeting?.llm_provider_id != null,
    progressActive,
  });

  const savedAtLabel = (at: Date) =>
    at.toLocaleTimeString(i18n.language, {
      hour: "2-digit",
      minute: "2-digit",
    });

  // ---- Empty states ---------------------------------------------------------

  if (!resolved) {
    return (
      <div
        dir={direction}
        className="flex h-screen items-center justify-center bg-background text-text"
      >
        <LoaderCircle className="animate-spin text-mid-gray" size={20} />
      </div>
    );
  }

  if (meetingId === null || (notFound && meeting === null)) {
    return (
      <div
        dir={direction}
        className="flex h-screen items-center justify-center bg-background p-6 text-center text-sm text-text-muted"
      >
        {notFound
          ? t("meeting.window.notFound")
          : t("meeting.window.noMeeting")}
      </div>
    );
  }

  if (meeting === null) {
    return (
      <div
        dir={direction}
        className="flex h-screen items-center justify-center bg-background text-text"
      >
        <LoaderCircle className="animate-spin text-mid-gray" size={20} />
      </div>
    );
  }

  const statusPill: Record<string, string> = {
    recording: "bg-recording-soft text-recording",
    paused: "bg-processing-soft text-processing",
    processing: "bg-processing-soft text-processing",
    ready: "bg-success-soft text-success",
    error: "bg-error-soft text-error",
    recovered: "bg-success-soft text-success",
  };

  const tabDefs: Array<{ id: Tab; icon: React.ReactNode; label: string }> = [
    {
      id: "notes",
      icon: <NotebookPen size={14} aria-hidden="true" />,
      label: t("meeting.window.tabs.notes"),
    },
    {
      id: "transcript",
      icon: <FileText size={14} aria-hidden="true" />,
      label: t("meeting.window.tabs.transcript"),
    },
    {
      id: "summary",
      icon: <Sparkles size={14} aria-hidden="true" />,
      label: t("meeting.window.tabs.summary"),
    },
  ];

  return (
    <div
      dir={direction}
      className="flex h-screen flex-col bg-background text-text"
    >
      {/* Header — FR-009-12: editable title, app, timer, status, pause/stop. */}
      <header className="flex items-center gap-3 border-b border-border px-4 py-3">
        <div className="min-w-0 flex-1">
          {editingTitle ? (
            <input
              ref={titleInputRef}
              type="text"
              value={titleDraft}
              maxLength={TITLE_MAX_CHARS}
              aria-label={t("meeting.window.editTitle")}
              placeholder={t("meeting.window.titlePlaceholder")}
              className="w-full rounded-md border border-border bg-surface px-2 py-1 text-sm font-semibold outline-none focus:border-accent"
              onChange={(e) => setTitleDraft(e.target.value)}
              onBlur={() => void commitTitle()}
              onKeyDown={(e) => {
                if (e.key === "Enter") void commitTitle();
                if (e.key === "Escape") setEditingTitle(false);
              }}
            />
          ) : (
            <button
              type="button"
              className="group flex w-full items-center gap-1.5 truncate text-start text-sm font-semibold hover:text-accent-hover"
              aria-label={t("meeting.window.editTitle")}
              title={meeting.title}
              onClick={() => {
                setTitleDraft(meeting.title);
                setEditingTitle(true);
              }}
            >
              <span className="truncate">{meeting.title}</span>
              <Pencil
                size={12}
                aria-hidden="true"
                className="shrink-0 opacity-0 transition-opacity group-hover:opacity-60"
              />
            </button>
          )}
          <div className="mt-0.5 flex items-center gap-2 text-xs text-mid-gray">
            {meeting.app_label ? (
              <span className="inline-flex items-center gap-1">
                <MonitorPlay size={12} aria-hidden="true" />
                <span className="truncate">{meeting.app_label}</span>
              </span>
            ) : meeting.detection === "in_person" ? (
              <span className="inline-flex items-center gap-1">
                <User size={12} aria-hidden="true" />
                {t("meeting.window.inPerson")}
              </span>
            ) : null}
            <span className="font-mono tabular-nums">
              {formatElapsedMs(elapsedMs)}
            </span>
          </div>
        </div>

        <span
          className={`rounded-full px-2 py-0.5 text-xs font-medium ${statusPill[status]}`}
        >
          {t(`meeting.window.status.${status}`)}
        </span>

        {live && (
          <>
            <button
              type="button"
              className="flex items-center gap-1 rounded-md border border-border px-2 py-1 text-xs font-medium hover:bg-accent-soft"
              aria-label={
                status === "paused"
                  ? t("meeting.window.resume")
                  : t("meeting.window.pause")
              }
              onClick={togglePause}
            >
              {status === "paused" ? (
                <Play size={12} aria-hidden="true" />
              ) : (
                <Pause size={12} aria-hidden="true" />
              )}
              {status === "paused"
                ? t("meeting.window.resume")
                : t("meeting.window.pause")}
            </button>
            <button
              type="button"
              className="flex items-center gap-1 rounded-md border border-error/40 bg-error-soft px-2 py-1 text-xs font-medium text-error hover:bg-error/10"
              aria-label={t("meeting.window.stop")}
              title={t("meeting.window.stopHint")}
              onClick={stopMeeting}
            >
              <Square size={10} aria-hidden="true" />
              {t("meeting.window.stop")}
            </button>
          </>
        )}
      </header>

      {/* Tabs — "Minhas notas" is the default focus (FR-009-13). */}
      <div
        role="tablist"
        className="flex gap-1 border-b border-border px-4 pt-2"
        onKeyDown={(e) => {
          // APG tablist: Left/Right (honoring document direction) cycle tabs.
          if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
          const order: Tab[] = ["notes", "transcript", "summary"];
          const delta =
            (e.key === "ArrowRight") !== (direction === "rtl") ? 1 : -1;
          const next =
            order[(order.indexOf(tab) + delta + order.length) % order.length];
          setTab(next);
          document.getElementById(`meeting-tab-${next}`)?.focus();
        }}
      >
        {tabDefs.map((def) => (
          <button
            key={def.id}
            type="button"
            role="tab"
            id={`meeting-tab-${def.id}`}
            aria-selected={tab === def.id}
            aria-controls={`meeting-panel-${def.id}`}
            tabIndex={tab === def.id ? 0 : -1}
            className={`flex items-center gap-1.5 rounded-t-md border-b-2 px-3 py-1.5 text-xs font-medium ${
              tab === def.id
                ? "border-accent text-text"
                : "border-transparent text-mid-gray hover:text-text"
            }`}
            onClick={() => setTab(def.id)}
          >
            {def.icon}
            {def.label}
          </button>
        ))}
      </div>

      {/* Panels */}
      <div className="min-h-0 flex-1">
        {tab === "notes" && (
          <div
            role="tabpanel"
            id="meeting-panel-notes"
            aria-labelledby="meeting-tab-notes"
            className="flex h-full flex-col p-4"
          >
            <textarea
              value={notesMd}
              onChange={(e) => setNotesMd(e.target.value)}
              placeholder={t("meeting.window.notesPlaceholder")}
              aria-label={t("meeting.window.tabs.notes")}
              className="min-h-0 flex-1 resize-none rounded-md border border-border bg-surface p-3 font-mono text-sm leading-relaxed outline-none focus:border-accent"
            />
            <div className="mt-2 flex items-center justify-between gap-3 text-xs text-mid-gray">
              <span className="truncate">{t("meeting.window.notesHint")}</span>
              <span className="shrink-0" role="status">
                {notesFailed
                  ? t("meeting.window.saveFailed")
                  : notesSaving
                    ? t("meeting.window.saving")
                    : notesSavedAt !== null
                      ? t("meeting.window.savedAt", {
                          time: savedAtLabel(notesSavedAt),
                        })
                      : ""}
              </span>
            </div>
          </div>
        )}

        {tab === "transcript" && (
          <div
            role="tabpanel"
            id="meeting-panel-transcript"
            aria-labelledby="meeting-tab-transcript"
            ref={transcriptScrollRef}
            onScroll={(e) => {
              const el = e.currentTarget;
              pinnedRef.current =
                el.scrollHeight - el.scrollTop - el.clientHeight <= 16;
            }}
            className="h-full overflow-y-auto p-4"
          >
            {visibleSegments.length === 0 ? (
              <p className="text-sm text-mid-gray">
                {t("meeting.window.transcriptEmpty")}
              </p>
            ) : (
              <ol className="space-y-2">
                {visibleSegments.map((seg) => {
                  const speaker = segmentSpeaker(seg);
                  const isMarker =
                    speaker.kind === "dictation" || speaker.kind === "gap";
                  return (
                    <li
                      key={seg.id}
                      className={`flex items-start gap-2 text-sm ${
                        seg.is_final ? "" : "opacity-70"
                      }`}
                    >
                      <span className="mt-0.5 shrink-0 font-mono text-xs text-mid-gray tabular-nums">
                        {formatSegmentTimestamp(seg.start_ms)}
                      </span>
                      <span className="mt-0.5 inline-flex shrink-0 items-center gap-1 text-xs font-medium text-text-muted">
                        {speaker.kind === "you" ? (
                          <User size={11} aria-hidden="true" />
                        ) : speaker.kind === "others" ||
                          speaker.kind === "named" ? (
                          <Users size={11} aria-hidden="true" />
                        ) : null}
                        {speakerLabel(seg)}
                        {!seg.is_final && (
                          <span className="text-mid-gray">
                            · {t("meeting.window.transcriptNotFinal")}
                          </span>
                        )}
                      </span>
                      <span
                        className={`min-w-0 flex-1 ${
                          isMarker ? "italic text-mid-gray" : "text-text"
                        }`}
                      >
                        {seg.text}
                      </span>
                    </li>
                  );
                })}
              </ol>
            )}
            <div ref={transcriptEndRef} />
          </div>
        )}

        {tab === "summary" && (
          <div
            role="tabpanel"
            id="meeting-panel-summary"
            aria-labelledby="meeting-tab-summary"
            className="flex h-full flex-col overflow-y-auto p-4"
          >
            {summaryPhase === "generating" && (
              <div className="flex items-center gap-2 text-sm text-mid-gray">
                <LoaderCircle size={14} className="animate-spin" />
                <span>{t("meeting.window.summary.generating")}</span>
              </div>
            )}

            {summaryPhase === "pending" && (
              <p className="text-sm text-mid-gray">
                {t("meeting.window.summary.pending")}
              </p>
            )}

            {summaryPhase === "disabled" && (
              <div className="flex items-start gap-2 rounded-md border border-warning/40 bg-warning-soft p-3 text-sm text-text">
                <TriangleAlert
                  size={14}
                  aria-hidden="true"
                  className="mt-0.5 shrink-0 text-warning"
                />
                <p>{t("meeting.window.summary.disabled")}</p>
              </div>
            )}

            {summaryPhase === "error" && (
              <div className="flex items-start gap-2 rounded-md border border-error/40 bg-error-soft p-3 text-sm text-text">
                <TriangleAlert
                  size={14}
                  aria-hidden="true"
                  className="mt-0.5 shrink-0 text-error"
                />
                <div className="flex flex-col gap-2">
                  <p>{t("meeting.window.summary.error")}</p>
                  <button
                    type="button"
                    className="self-start rounded-md border border-border px-2 py-1 text-xs font-medium hover:bg-accent-soft"
                    disabled={retryingSummary}
                    onClick={() => void retrySummary()}
                  >
                    {t("meeting.window.summary.retry")}
                  </button>
                </div>
              </div>
            )}

            {summaryPhase === "ready" && (
              <div className="flex min-h-0 flex-1 flex-col gap-2">
                <div className="flex items-center justify-between gap-2">
                  <p className="text-xs text-mid-gray">
                    {t("meeting.window.summary.editHint")}
                  </p>
                  <div className="flex items-center gap-2">
                    <span className="text-xs text-mid-gray" role="status">
                      {summarySaving
                        ? t("meeting.window.saving")
                        : summarySavedAt !== null
                          ? t("meeting.window.savedAt", {
                              time: savedAtLabel(summarySavedAt),
                            })
                          : ""}
                    </span>
                    <button
                      type="button"
                      className="rounded-md border border-border px-2 py-1 text-xs font-medium hover:bg-accent-soft"
                      aria-label={
                        summaryEditing
                          ? t("meeting.window.summary.view")
                          : t("meeting.window.summary.edit")
                      }
                      onClick={() => {
                        if (summaryEditing) {
                          setSummaryEditing(false);
                          setSummaryDraft(null);
                        } else {
                          setSummaryDraft(meeting.summary_md ?? "");
                          setSummaryEditing(true);
                        }
                      }}
                    >
                      {summaryEditing
                        ? t("meeting.window.summary.view")
                        : t("meeting.window.summary.edit")}
                    </button>
                  </div>
                </div>
                {summaryEditing ? (
                  <textarea
                    value={summaryDraft ?? ""}
                    onChange={(e) => setSummaryDraft(e.target.value)}
                    aria-label={t("meeting.window.tabs.summary")}
                    className="min-h-0 flex-1 resize-none rounded-md border border-border bg-surface p-3 font-mono text-sm leading-relaxed outline-none focus:border-accent"
                  />
                ) : (
                  <div className="min-h-0 flex-1 overflow-y-auto rounded-md border border-border bg-surface p-3">
                    <MarkdownContent markdown={meeting.summary_md ?? ""} />
                  </div>
                )}
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
};

export default MeetingWindow;
