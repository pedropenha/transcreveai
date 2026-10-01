import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import React, {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import {
  Check,
  CircleDot,
  Mic,
  Pause,
  Play,
  Square,
  TriangleAlert,
  X,
} from "lucide-react";
import "./RecordingOverlay.css";
import { commands, events } from "@/bindings";
import type {
  MeetingStateEvent,
  StreamPhase,
  StreamPhaseEvent,
  StreamTextEvent,
  StreamWorkKind,
} from "@/bindings";
import i18n, { syncLanguageFromSettings } from "@/i18n";
import { getLanguageDirection } from "@/lib/utils/rtl";
import { formatKeyCombination } from "@/lib/utils/keyboard";
import { useOsType } from "@/hooks/useOsType";
import {
  effectiveEdge,
  meetingStateClaimsFlowbar,
  resolveFlowbarView,
  toastBadgeVisible,
  type FlowbarView,
  type MeetingStatus,
  type OverlayHint,
  type SessionPhase,
  type StageEdge,
} from "./flowbarView";
import type { ToastStateEvent } from "@/toast/toastView";

// Number of reactive bars in the waveform (the simple, smoothed style shared by
// every overlay form). Mic levels arrive as 16 FFT buckets; we take the first N.
// F001/T-008: the recording pill shows 7 bars.
const WAVE_BARS = 7;
// FFT room tone still produces a small positive level. Treat the calibrated
// meter's lowest bucket range as silence so only speech-level input lifts bars.
const MIN_VISIBLE_LEVEL = 0.08;

// FR-001-02: hover opens after a 120 ms delay; FR-001-02 note: leaving is
// lazier (400 ms) so crossing to the tooltip or wobbling over the slit edge
// doesn't collapse the card.
const HOVER_ENTER_MS = 120;
const HOVER_LEAVE_MS = 400;

/** `session://state` payload (transcription_coordinator/machine.rs). */
interface SessionStatePayload {
  session_id: string | null;
  state: SessionPhase;
  mode: string;
  error?: string | null;
  notice?: string | null;
  pending: number;
}

const RecordingOverlay: React.FC = () => {
  const { t } = useTranslation();
  const osType = useOsType();

  // --- Presence (F001 §Sempre + legacy show/hide) ---
  // `alwaysOn`: flowbar_visibility === "always" && overlay enabled — the idle
  // slit lives on screen between sessions. `windowActive`: a show-overlay is
  // currently in effect (session-driven or the startup presence hint).
  const [alwaysOn, setAlwaysOn] = useState(false);
  const [windowActive, setWindowActive] = useState(false);
  const [hint, setHint] = useState<OverlayHint | null>(null);

  // --- Session lifecycle (coordinator `session://state`) ---
  const [phase, setPhase] = useState<SessionPhase>("idle");
  const [notice, setNotice] = useState<string | null>(null);
  const [sessionError, setSessionError] = useState<string | null>(null);
  const [retrying, setRetrying] = useState(false);

  // --- Meeting lifecycle (`meeting://state`, T-064 / FR-009-07) ---
  const [meetingStatus, setMeetingStatus] = useState<MeetingStatus>("idle");
  const [meetingElapsed, setMeetingElapsed] = useState(0);
  const [meetingId, setMeetingId] = useState<string | null>(null);
  const [meetingCaptureReady, setMeetingCaptureReady] = useState(false);
  const [meetingLevels, setMeetingLevels] = useState<number[]>(
    Array(WAVE_BARS).fill(0),
  );

  // --- Hover / click-through (FR-001-02, NFR-001-02) ---
  const [hovered, setHovered] = useState(false);
  const [tip, setTip] = useState<"dictate" | "notetaker" | "error" | null>(
    null,
  );

  // --- Dock edge the stage mirrors (FR-001-01/08) ---
  const [edge, setEdge] = useState<StageEdge>("bottom");
  const [dictateShortcut, setDictateShortcut] = useState<string>("");

  // --- Collapsed/suppressed meeting toast → amber dot (FR-008-10/12) ---
  const [toastPending, setToastPending] = useState(false);

  // `Stream::play()` returning does not mean hardware callbacks are flowing.
  // Stay visually in an arming state until the backend processes the first
  // actual microphone sample chunk.
  const [captureReady, setCaptureReady] = useState(false);
  const [levels, setLevels] = useState<number[]>(Array(WAVE_BARS).fill(0));
  const [streamText, setStreamText] = useState<StreamTextEvent>({
    committed: "",
    tentative: "",
  });
  const [streamPhase, setStreamPhase] = useState<StreamPhase>("listening");
  const [workKind, setWorkKind] = useState<StreamWorkKind>("transcribing");
  const [elapsed, setElapsed] = useState(0);
  // Bumped on each new streaming session so the Live card remounts fresh (replays
  // the pop-in, and never animates in from the previous panel's open size).
  const [session, setSession] = useState(0);
  // True once live text overflows the cap. A top overlay fades its top edge only
  // while overflowing, so the resting first line stays crisp flush under the pill.
  const [overflowing, setOverflowing] = useState(false);

  const smoothedLevelsRef = useRef<number[]>(Array(16).fill(0));
  const meetingSmoothedLevelsRef = useRef<number[]>(Array(16).fill(0));
  const meetingStatusRef = useRef<MeetingStatus>("idle");
  const meetingIdRef = useRef<string | null>(null);
  const overlayEventGenerationRef = useRef(0);
  const enterTimerRef = useRef<number | undefined>(undefined);
  const leaveTimerRef = useRef<number | undefined>(undefined);
  // The interactive surface the core keeps clickable (rest of the window stays
  // click-through, NFR-001-02). Wraps the card + its tooltip so both get events.
  const zoneRef = useRef<HTMLDivElement>(null);
  // Live-text scroll-back: the text region "sticks" to the newest line while the
  // user is at the bottom; if they scroll up to read history, auto-follow pauses
  // until they scroll back down.
  const capRef = useRef<HTMLDivElement>(null);
  const pinnedRef = useRef(true);
  const direction = getLanguageDirection(i18n.language);

  // Presence + shortcut read: which settings gate the slit and what the Ditar
  // tooltip advertises (FR-001-03 — read from the configured binding, never
  // hardcoded).
  const refreshSettings = useCallback(async () => {
    try {
      const settings = await commands.getAppSettings();
      if (settings.status !== "ok") return;
      const s = settings.data;
      setAlwaysOn(
        s.flowbar_visibility === "always" && s.overlay_style !== "none",
      );
      setEdge(effectiveEdge(s.flowbar_position_edge, s.overlay_position));
      setDictateShortcut(
        formatKeyCombination(
          s.bindings?.["transcribe"]?.current_binding ?? "",
          osType,
        ),
      );
    } catch {
      // Keep the previous/default placement if settings can't be read.
    }
  }, [osType]);

  // Hover timers — cursor entry is mostly signalled by `flowbar://cursor`
  // (the DOM can't see a click-through window), DOM enter/leave is the backup
  // path while the window is already interactive.
  const clearHoverTimers = useCallback(() => {
    window.clearTimeout(enterTimerRef.current);
    window.clearTimeout(leaveTimerRef.current);
    enterTimerRef.current = undefined;
    leaveTimerRef.current = undefined;
  }, []);
  const scheduleEnter = useCallback(() => {
    window.clearTimeout(leaveTimerRef.current);
    if (enterTimerRef.current === undefined) {
      enterTimerRef.current = window.setTimeout(() => {
        enterTimerRef.current = undefined;
        setHovered(true);
      }, HOVER_ENTER_MS);
    }
  }, []);
  const scheduleLeave = useCallback(() => {
    window.clearTimeout(enterTimerRef.current);
    enterTimerRef.current = undefined;
    window.clearTimeout(leaveTimerRef.current);
    leaveTimerRef.current = window.setTimeout(() => {
      setHovered(false);
      setTip(null);
    }, HOVER_LEAVE_MS);
  }, []);

  useEffect(() => {
    refreshSettings();

    const setupEventListeners = async () => {
      const unlistenShow = await listen("show-overlay", async (event) => {
        const generation = ++overlayEventGenerationRef.current;
        const overlayHint = event.payload as OverlayHint;
        // Reset synchronously before settings I/O. A fast microphone can emit
        // recording-ready while the awaits below are in flight; resetting after
        // them would overwrite that event and leave the overlay stuck arming.
        if (overlayHint === "recording" || overlayHint === "streaming") {
          setCaptureReady(false);
          smoothedLevelsRef.current = Array(16).fill(0);
          setLevels(Array(WAVE_BARS).fill(0));
          setStreamText({ committed: "", tentative: "" });
        }

        await syncLanguageFromSettings();
        await refreshSettings();
        if (generation !== overlayEventGenerationRef.current) return;
        setHint(overlayHint);
        if (overlayHint === "streaming") {
          setStreamPhase("listening");
          setWorkKind("transcribing");
          setElapsed(0);
          setSession((s) => s + 1); // remount the card fresh for this session
        }
        setWindowActive(true);
      });

      const unlistenHide = await listen("hide-overlay", () => {
        overlayEventGenerationRef.current += 1;
        setWindowActive(false);
        setHint(null);
        setNotice(null);
        setCaptureReady(false);
        clearHoverTimers();
        setHovered(false);
        setTip(null);
      });

      // Coordinator lifecycle — the authoritative state vocabulary (F001).
      const unlistenSession = await listen<SessionStatePayload>(
        "session://state",
        (event) => {
          const payload = event.payload;
          setPhase(payload.state);
          setNotice(payload.notice ?? null);
          setSessionError(
            payload.state === "error" ? (payload.error ?? null) : null,
          );
          if (payload.state === "idle") setRetrying(false);
        },
      );

      // Cursor crossing the interactive rect while the window flips between
      // click-through and interactive (NFR-001-02). `false` arrives when the
      // window went click-through mid-hover — the DOM may never see a leave.
      const unlistenCursor = await listen<boolean>(
        "flowbar://cursor",
        (event) => {
          if (event.payload) {
            scheduleEnter();
          } else {
            scheduleLeave();
          }
        },
      );

      const unlistenReady = await listen("recording-ready", () => {
        setElapsed(0);
        setCaptureReady(true);
      });

      const unlistenLevel = await listen<{ rms: number[] }>(
        "audio://level",
        (event) => {
          const newLevels = event.payload.rms;
          setCaptureReady(true);
          // Exponential smoothing across the 16 buckets, then take the first N
          // bars for the shared waveform.
          const smoothed = smoothedLevelsRef.current.map((prev, i) => {
            const target = newLevels[i] || 0;
            return prev * 0.7 + target * 0.3;
          });
          smoothedLevelsRef.current = smoothed;
          setLevels(smoothed.slice(0, WAVE_BARS));
          if (meetingStatusRef.current === "recording") {
            const meetingSmoothed = meetingSmoothedLevelsRef.current.map(
              (previous, index) =>
                previous * 0.7 + (newLevels[index] || 0) * 0.3,
            );
            meetingSmoothedLevelsRef.current = meetingSmoothed;
            setMeetingCaptureReady(true);
            setMeetingLevels(meetingSmoothed.slice(0, WAVE_BARS));
          }
        },
      );

      // Meeting session — the always-visible recording indicator (FR-009-07).
      const applyMeetingState = (payload: MeetingStateEvent): boolean => {
        const status = payload.status;
        if (
          !meetingStateClaimsFlowbar(
            meetingStatusRef.current,
            meetingIdRef.current,
            status,
            payload.meeting_id,
          )
        ) {
          return false;
        }
        setMeetingElapsed(Math.floor(payload.elapsed_ms / 1000));
        const knownStatus: MeetingStatus = (
          [
            "recording",
            "paused",
            "processing",
            "ready",
            "error",
            "recovered",
          ] as readonly string[]
        ).includes(status)
          ? (status as MeetingStatus)
          : "idle";
        const live = knownStatus === "recording" || knownStatus === "paused";
        if (
          meetingStatusRef.current !== "recording" &&
          knownStatus === "recording"
        ) {
          setMeetingCaptureReady(false);
          meetingSmoothedLevelsRef.current = Array(16).fill(0);
          setMeetingLevels(Array(WAVE_BARS).fill(0));
        } else if (!live) {
          setMeetingCaptureReady(false);
        }
        meetingStatusRef.current = knownStatus;
        setMeetingStatus(knownStatus);
        // Terminal faces keep the id for the short dwell so clicking the
        // outcome opens the finished meeting's transcript.
        const nextMeetingId =
          knownStatus === "idle" ? null : payload.meeting_id;
        meetingIdRef.current = nextMeetingId;
        setMeetingId(nextMeetingId);
        return true;
      };
      let meetingEventSeen = false;
      const unlistenMeeting = await listen<MeetingStateEvent>(
        "meeting://state",
        (event) => {
          meetingEventSeen = applyMeetingState(event.payload);
        },
      );

      const unlistenStream = await events.streamTextEvent.listen((event) => {
        setStreamText(event.payload);
      });

      const unlistenPhase = await events.streamPhaseEvent.listen((event) => {
        const payload: StreamPhaseEvent = event.payload;
        setStreamPhase(payload.phase);
        if (payload.kind) setWorkKind(payload.kind);
      });

      // F008/T-062: `toast://state` is broadcast to every window; the Flow Bar
      // only cares that a collapsed toast still has content → amber dot.
      const unlistenToast = await listen<ToastStateEvent>(
        "toast://state",
        (event) => {
          const payload = event.payload;
          setToastPending(
            payload.collapsed &&
              (payload.detection !== null || payload.notice !== null),
          );
        },
      );

      try {
        const currentMeeting = await commands.meetingCurrent();
        if (
          !meetingEventSeen &&
          currentMeeting.status === "ok" &&
          currentMeeting.data
        ) {
          applyMeetingState(currentMeeting.data);
        }
      } catch (error) {
        console.warn("Failed to hydrate meeting state", error);
      }

      return () => {
        unlistenShow();
        unlistenHide();
        unlistenSession();
        unlistenCursor();
        unlistenReady();
        unlistenLevel();
        unlistenMeeting();
        unlistenStream();
        unlistenPhase();
        unlistenToast();
      };
    };

    let cleanup: (() => void) | undefined;
    let disposed = false;
    setupEventListeners()
      .then((fn) => {
        if (disposed) fn();
        else cleanup = fn;
      })
      .catch((error) => console.error("Failed to initialize Flow Bar", error));
    return () => {
      disposed = true;
      cleanup?.();
      clearHoverTimers();
    };
    // Mount-once: every callback the listeners use is stable (useCallback) or
    // a setState, and refreshSettings is re-run inside the show handler.
  }, []);

  const view: FlowbarView = resolveFlowbarView({
    alwaysOn,
    windowActive,
    phase,
    meeting: meetingStatus,
    notice,
    hint,
    hovered,
    retrying,
  });

  // Terminal meeting outcomes get the same finite dwell as the dictation's
  // done/error faces; `processing` stays up until the backend emits its
  // terminal state.
  useEffect(() => {
    if (
      meetingStatus !== "ready" &&
      meetingStatus !== "recovered" &&
      meetingStatus !== "error"
    ) {
      return;
    }
    const status = meetingStatus;
    const timer = window.setTimeout(() => {
      if (meetingStatusRef.current !== status) return;
      meetingStatusRef.current = "idle";
      setMeetingStatus("idle");
      meetingIdRef.current = null;
      setMeetingId(null);
    }, 4_000);
    return () => window.clearTimeout(timer);
  }, [meetingStatus, meetingId]);

  // Report the interactive rect to the core so everything outside it stays
  // click-through (contracts.md §5 `flowbar_set_hover`, bounds form). Re-
  // measured once the morph transition settles — the pill changes size per
  // view (F001: fixed size per visual class, the window itself never moves).
  useLayoutEffect(() => {
    const el = zoneRef.current;
    const report = () => {
      if (!el || view === "hidden") {
        void commands.flowbarSetHover(null);
        return;
      }
      const r = el.getBoundingClientRect();
      void commands.flowbarSetHover({
        x: r.x,
        y: r.y,
        width: r.width,
        height: r.height,
      });
    };
    report();
    const settle = window.setTimeout(report, 240);
    let frame = 0;
    const observer =
      typeof ResizeObserver === "undefined" || !el
        ? null
        : new ResizeObserver(() => {
            window.cancelAnimationFrame(frame);
            frame = window.requestAnimationFrame(report);
          });
    if (el) observer?.observe(el);
    return () => {
      window.clearTimeout(settle);
      window.cancelAnimationFrame(frame);
      observer?.disconnect();
    };
  }, [view, tip, hovered, toastPending]);

  // A tooltip belongs to the hover/error card — drop it the moment the bar
  // morphs into another state, otherwise "Ditar · Ctrl+Win" would linger over
  // the recording pill while the cursor sits still inside it.
  useEffect(() => {
    if (view !== "hover" && view !== "error" && tip !== null) setTip(null);
  }, [view, tip]);

  // Elapsed capture timer starts only once microphone samples are flowing.
  useEffect(() => {
    if (view !== "streaming" || !captureReady) return;
    const id = setInterval(() => setElapsed((e) => e + 1), 1000);
    return () => clearInterval(id);
  }, [view, captureReady]);

  // Stick to the bottom as text streams in — but only while pinned, so a user who
  // has scrolled up to read history isn't yanked back down by the next chunk.
  useLayoutEffect(() => {
    const el = capRef.current;
    if (!el) return;
    // Fade the top edge only once text actually overflows the cap.
    setOverflowing(el.scrollHeight > el.clientHeight + 1);
    if (pinnedRef.current) el.scrollTop = el.scrollHeight;
  }, [streamText]);

  // Each fresh streaming session starts pinned to the bottom, fade cleared.
  useEffect(() => {
    pinnedRef.current = true;
    setOverflowing(false);
  }, [session]);

  const handleStreamScroll = () => {
    const el = capRef.current;
    if (!el) return;
    pinnedRef.current = el.scrollHeight - el.scrollTop - el.clientHeight <= 16;
  };

  const fmtTime = (s: number) =>
    `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;

  // ---- Actions -------------------------------------------------------------

  const dictate = () => {
    // Same toggle edge as the configured shortcut — starts hands-free from
    // idle, ends-and-inserts while recording (FR-001-03/06).
    void commands.flowbarToggleDictation();
  };
  const notetaker = () => {
    void commands.flowbarStartNotetaker();
  };
  // FR-009-06: the meeting pill's own pause/resume + stop.
  const meetingTogglePause = () => {
    void (meetingStatus === "paused"
      ? commands.meetingResume()
      : commands.meetingPause());
  };
  const meetingStop = () => {
    void commands.meetingStop();
  };
  // FR-009-14: the pill's timer is the Flow Bar reopen surface for the
  // (hide-on-close) meeting window.
  const meetingOpen = () => {
    void commands.meetingWindowOpen(meetingId);
  };
  const cancel = () => {
    void commands.cancelOperation();
  };
  const retry = () => {
    setRetrying(true);
    void commands.flowbarRetryLastFailed().then((result) => {
      // A rejected retry keeps the error face — the dwell or next session
      // event resolves it. (Backend refusal = no failed row left, etc.)
      if (result.status !== "ok") setRetrying(false);
    });
  };

  // ---- Shared building blocks (one visual language for every overlay form) ----

  const renderWaveform = (
    paused = false,
    ready = captureReady,
    values = levels,
  ) => (
    <div className={`swave ${paused ? "paused" : ready ? "ready" : "arming"}`}>
      {values.map((v, i) => {
        const level = v < MIN_VISIBLE_LEVEL ? 0 : v;
        return (
          <i
            key={i}
            style={{
              height: `${Math.max(
                3,
                Math.min(18, 3 + Math.pow(level, 0.7) * 15),
              )}px`,
            }}
          />
        );
      })}
    </div>
  );
  const waveform = renderWaveform();

  const cancelBtn = (
    <button className="sx" aria-label={t("overlay.cancel")} onClick={cancel}>
      <X size={10} aria-hidden="true" />
    </button>
  );

  // dot (left) | waveform (center) | timer + cancel (right) — Live panel layout.
  const listeningRow = (showTimer: boolean, showCancel: boolean) => (
    <div className="sbase">
      <div className="sbase-l">
        <span className={`sdot ${captureReady ? "ready" : "arming"}`} />
      </div>
      {waveform}
      <div className="sbase-r">
        {showTimer && <span className="stimer">{fmtTime(elapsed)}</span>}
        {showCancel && cancelBtn}
      </div>
    </div>
  );

  // spinner (left) | label (center) | cancel (right) — same 3-zone grid as the
  // listening row, so the label is centered.
  const workingRow = (label: string, showCancel: boolean) => (
    <div className="sbase">
      <div className="sbase-l">
        <span className="sspinner" />
      </div>
      <span className="swork-label">{label}</span>
      <div className="sbase-r">{showCancel && cancelBtn}</div>
    </div>
  );

  // Announce visual state changes to assistive tech without announcing every
  // mic sample (T-008): only the view-level label lands in the live region.
  const announce = (() => {
    switch (view) {
      case "recording":
        return t("overlay.listening");
      case "meeting-recording":
        return meetingStatus === "paused"
          ? t("overlay.meetingPaused")
          : t("overlay.meetingRecording");
      case "working":
        if (meetingStatus === "processing" && phase === "idle") {
          return t("overlay.meetingProcessing");
        }
        if (retrying) return t("overlay.retrying");
        if (phase === "inserting") return t("overlay.inserting");
        return phase === "processing"
          ? t("overlay.processing")
          : t("overlay.transcribing");
      case "done":
        if (phase === "idle" && meetingStatus === "recovered") {
          return t("overlay.meetingRecovered");
        }
        if (phase === "idle" && meetingStatus === "ready") {
          return t("overlay.meetingReady");
        }
        return t("overlay.done");
      case "error":
        if (phase === "idle" && meetingStatus === "error") {
          return t("overlay.meetingFailed");
        }
        return sessionError || t("overlay.failed");
      case "nothing-heard":
        return t("overlay.nothingHeard");
      default:
        return "";
    }
  })();

  const dictateTip =
    dictateShortcut === ""
      ? t("overlay.dictate")
      : `${t("overlay.dictate")} · ${dictateShortcut}`;

  const tipContent = (() => {
    if (tip === "dictate") return dictateTip;
    if (tip === "notetaker") return t("overlay.notetaker");
    if (tip === "error") return sessionError || t("overlay.failed");
    return null;
  })();

  // ---- Live overlay: a pill that sculpts open into a panel ----
  const renderStreaming = () => {
    const hasText =
      streamText.committed.length > 0 || streamText.tentative.length > 0;
    const working = streamPhase === "working";
    // Keep the panel open whenever there's text — even while finalizing — so the
    // transcript stays put under a working spinner instead of collapsing and
    // squishing the text mid-stream. Only fall back to the small working pill
    // when there was no text to preserve.
    const open = hasText;
    const collapsed = working && !hasText;

    return (
      <div
        key={session}
        className={`scard ${open ? "open" : ""} ${collapsed ? "working" : ""} ${
          windowActive ? "" : "leaving"
        }`}
      >
        <div className="stext">
          <div className="stext-clip">
            <div
              className={`stext-cap ${overflowing ? "overflowing" : ""}`}
              ref={capRef}
              onScroll={handleStreamScroll}
            >
              <p>
                <span className="committed">
                  {streamText.committed ? streamText.committed + " " : ""}
                </span>
                <span className="tentative">{streamText.tentative}</span>
                {/* Drop the blinking caret once finalizing — it's no longer
                    capturing, and a static spinner conveys the work. */}
                {!working && <span className="scaret" />}
              </p>
            </div>
          </div>
        </div>
        {working
          ? workingRow(
              workKind === "polishing"
                ? t("overlay.processing")
                : t("overlay.transcribing"),
              true,
            )
          : listeningRow(open, true)}
      </div>
    );
  };

  // ---- Compact Flow Bar states (F001) --------------------------------------

  const renderCompact = () => {
    switch (view) {
      case "idle":
        // 48x8 slit — "quase invisível". The invisible hitbox padding around
        // it is what the webview reports to the hit-test (T-008: enlarge the
        // hit area without changing the visible silhouette).
        return (
          <div className="fbar-hitbox" aria-hidden="true">
            <div className="scard fbar-card f-idle" />
          </div>
        );

      case "hover":
        // FR-001-02/03/04: dictate + notetaker as two *separate* pills —
        // the mic control and the Notetaker dot are distinct surfaces, not
        // one merged card. The assistant is shortcut-only (F012: no Flow
        // Bar slot).
        return (
          <div className="f-hover-group">
            <div className="fbar-card scard f-hover-pill">
              <button
                type="button"
                className="fbtn"
                aria-label={dictateTip}
                onMouseEnter={() => setTip("dictate")}
                onFocus={() => setTip("dictate")}
                onMouseLeave={() => setTip(null)}
                onBlur={() => setTip(null)}
                onClick={dictate}
              >
                <Mic size={14} aria-hidden="true" />
              </button>
            </div>
            <div className="fbar-card scard f-hover-pill">
              <button
                type="button"
                className="fbtn"
                aria-label={t("overlay.notetaker")}
                onMouseEnter={() => setTip("notetaker")}
                onFocus={() => setTip("notetaker")}
                onMouseLeave={() => setTip(null)}
                onBlur={() => setTip(null)}
                onClick={notetaker}
              >
                <CircleDot size={14} aria-hidden="true" />
              </button>
            </div>
          </div>
        );

      case "recording":
        // F001: ✕ left, waveform center, ■ right. Flat bars while the mic is
        // muted/no-input because nothing louder than silence arrives (levels
        // come straight from `audio://level`; AC-001-07).
        return (
          <div className="scard fbar-card f-rec">
            <div className="frow">
              <button
                type="button"
                className="sx fside"
                aria-label={t("overlay.cancel")}
                onClick={cancel}
              >
                <X size={10} aria-hidden="true" />
              </button>
              {waveform}
              <button
                type="button"
                className="sx fside fstop"
                aria-label={t("overlay.stop")}
                onClick={dictate}
              >
                <Square size={9} aria-hidden="true" />
              </button>
            </div>
          </div>
        );

      case "meeting-recording": {
        // FR-009-06/07: same pill language as dictation — left pause/resume,
        // meeting timer center, ■ right stops the *meeting* (the dictation
        // pill's ■ ends-and-inserts, so this face only wins while idle).
        const paused = meetingStatus === "paused";
        return (
          <div className="scard fbar-card f-rec f-meeting">
            <div className="frow">
              <button
                type="button"
                className="sx fside"
                aria-label={paused ? t("overlay.resume") : t("overlay.pause")}
                onClick={meetingTogglePause}
              >
                {paused ? (
                  <Play size={10} aria-hidden="true" />
                ) : (
                  <Pause size={10} aria-hidden="true" />
                )}
              </button>
              <div className="fmeeting-center">
                {renderWaveform(paused, meetingCaptureReady, meetingLevels)}
                <button
                  type="button"
                  className="sx stimer"
                  aria-label={t("overlay.openMeetingAt", {
                    time: fmtTime(meetingElapsed),
                  })}
                  title={t("overlay.openMeeting")}
                  onClick={meetingOpen}
                >
                  {fmtTime(meetingElapsed)}
                </button>
              </div>
              <button
                type="button"
                className="sx fside fstop"
                aria-label={t("overlay.stopMeeting")}
                onClick={meetingStop}
              >
                <Square size={9} aria-hidden="true" />
              </button>
            </div>
          </div>
        );
      }

      case "working":
        // F001 processing shape: three pulsing dots + an AT-only label — the
        // actual phase text lives in the live region, not color or motion.
        if (meetingStatus === "processing" && phase === "idle") {
          return (
            <button
              type="button"
              className="scard fbar-card f-work f-meeting-state"
              aria-label={t("overlay.meetingProcessing")}
              onClick={meetingOpen}
            >
              <span className="frow frow-work">
                <span className="fdots" aria-hidden="true">
                  <i />
                  <i />
                  <i />
                </span>
                <span className="fbar-sr">
                  {t("overlay.meetingProcessing")}
                </span>
              </span>
            </button>
          );
        }
        return (
          <div className="scard fbar-card f-work">
            <div className="frow frow-work">
              <span className="fdots" aria-hidden="true">
                <i />
                <i />
                <i />
              </span>
              <span className="fbar-sr">
                {phase === "inserting"
                  ? t("overlay.inserting")
                  : retrying
                    ? t("overlay.retrying")
                    : phase === "processing"
                      ? t("overlay.processing")
                      : t("overlay.transcribing")}
              </span>
            </div>
          </div>
        );

      case "done":
        if (
          (meetingStatus === "ready" || meetingStatus === "recovered") &&
          phase === "idle"
        ) {
          return (
            <button
              type="button"
              className="scard fbar-card f-done f-meeting-state"
              aria-label={t("overlay.openMeetingTranscript")}
              onClick={meetingOpen}
            >
              <Check size={12} className="fdone-icon" aria-hidden="true" />
            </button>
          );
        }
        return (
          <div className="scard fbar-card f-done">
            <Check size={12} className="fdone-icon" aria-hidden="true" />
          </div>
        );

      case "error":
        if (meetingStatus === "error" && phase === "idle") {
          return (
            <button
              type="button"
              className="scard fbar-card f-error f-meeting-state"
              aria-label={t("overlay.meetingFailed")}
              onClick={meetingOpen}
            >
              <TriangleAlert
                size={12}
                className="ferror-icon"
                aria-hidden="true"
              />
              <span className="fbar-sr">{t("overlay.meetingFailed")}</span>
            </button>
          );
        }
        // AC-001-08: ⚠ pill; hovering reveals a concise cause + retry affordance.
        return (
          <div
            className="scard fbar-card f-error"
            role="button"
            tabIndex={0}
            aria-label={t("overlay.failed")}
            aria-describedby={tip === "error" ? "fbar-error-tip" : undefined}
            onMouseEnter={() => setTip("error")}
            onMouseLeave={() => setTip(null)}
            onFocus={() => setTip("error")}
            onBlur={(event) => {
              const zone = event.currentTarget.closest(".fbar-zone");
              if (!zone?.contains(event.relatedTarget as Node | null)) {
                setTip(null);
              }
            }}
            onKeyDown={(event) => {
              if (event.key === "Enter" || event.key === " ") {
                event.preventDefault();
                retry();
              }
            }}
          >
            <TriangleAlert
              size={12}
              className="ferror-icon"
              aria-hidden="true"
            />
            <span className="fbar-sr">{t("overlay.failed")}</span>
          </div>
        );

      case "nothing-heard":
        // FR-002-14: the session was discarded — a brief static label, no
        // waveform/spinner/cancel. Backend re-hides (or re-slits) after ~1 s.
        return (
          <div className="scard fbar-card f-heard">
            <span className="swork-label">{t("overlay.nothingHeard")}</span>
          </div>
        );

      default:
        return null;
    }
  };

  // The live region is mounted even while session-only mode hides the card;
  // otherwise a screen reader can miss a live region that appears already
  // populated on the first visible frame.
  return (
    <div dir={direction} className={`ov-stage ${edge} ov-fade show`}>
      {view !== "hidden" && (
        <div
          ref={zoneRef}
          className="fbar-zone"
          onMouseEnter={scheduleEnter}
          onMouseLeave={scheduleLeave}
        >
          {tipContent !== null && (
            <div
              id={tip === "error" ? "fbar-error-tip" : undefined}
              className="fbar-tip"
              role="tooltip"
            >
              <span>{tipContent}</span>
              {tip === "error" && (
                <button
                  type="button"
                  className="fbtn fretry"
                  onClick={retry}
                  aria-label={t("overlay.retry")}
                >
                  {t("overlay.retry")}
                </button>
              )}
            </div>
          )}
          {/* FR-008-10/12: collapsed/suppressed meeting toast → amber dot that
            reopens it on hover. In-flow inside the zone so the hit-test rect
            covers it (NFR-001-02 click-through otherwise eats the hover). */}
          {toastBadgeVisible(view, toastPending) && (
            <button
              type="button"
              className="fbar-toast-dot"
              aria-label={t("toast.meetingPending")}
              onMouseEnter={() => void invoke("toast_reopen")}
              onFocus={() => void invoke("toast_reopen")}
            />
          )}
          {view === "streaming" ? renderStreaming() : renderCompact()}
        </div>
      )}
      <span className="fbar-sr" aria-live="polite">
        {announce}
      </span>
    </div>
  );
};

export default RecordingOverlay;
