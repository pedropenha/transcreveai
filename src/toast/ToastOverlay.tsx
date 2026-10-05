import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { commands } from "@/bindings";
import type { CommandError, Result } from "@/bindings";
import React, { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { BellRing, TriangleAlert, X } from "lucide-react";
import MeetingCard from "./MeetingCard";
import "./ToastOverlay.css";
import i18n, { syncLanguageFromSettings } from "@/i18n";
import { getLanguageDirection } from "@/lib/utils/rtl";
import {
  COLLAPSE_AFTER_MS,
  CONFIRMATION_MS,
  NOTICE_AUTO_DISMISS_MS,
  noticeActionFor,
  noticeBlocksCollapse,
  noticeSelfDismisses,
  resolveToastView,
  startsRecording,
  toastWindowHeight,
  type DetectorAction,
  type NoticeAction,
  type NoticeActionButton,
  type ToastStateEvent,
} from "./toastView";

/**
 * The meeting toast (F008 / T-062): always-expanded detection card.
 * Everything user-facing comes from `toast://state`; the 60 s collapse timer
 * (FR-008-10) lives here — on expiry it reports `toast_set_collapsed` and the
 * backend hides the window + lights the Flow Bar's amber dot.
 */

/** Kinds that render the warning triangle rather than the bell —
 *  `toast://show.kind` is a semantic discriminator, not a style. */
function isWarningKind(kind: string | undefined): boolean {
  return (
    kind === "warning" ||
    kind === "error" ||
    kind === "translated_dictation_error" ||
    kind === "meeting_error" ||
    kind === "meeting_warning" ||
    kind === "meeting_limit" ||
    kind === "meeting_auto_stop"
  );
}

const TRANSLATION_ERROR_CODES = new Set([
  "translation_model_required",
  "translation_model_unavailable",
  "translation_model_incompatible",
  "translation_model_changed",
  "translation_target_changed",
  "translation_model_loading",
  "translation_meeting_active",
  "translation_insertion_unsupported",
  "translation_failed",
]);

/** Run a specta command: `{ status: "error" }` (backend refusal) and a
 *  thrown error both mean "didn't run" — return false and log either. */
async function ranCommand(
  name: string,
  call: () => Promise<Result<null, CommandError>>,
): Promise<boolean> {
  try {
    const res = await call();
    if (res.status === "error") {
      console.warn(`${name} refused:`, res.error);
      return false;
    }
    return true;
  } catch (e) {
    console.warn(`${name} failed:`, e);
    return false;
  }
}

async function detectorRespond(
  detectionId: string,
  action: DetectorAction,
): Promise<boolean> {
  // A refused response dismisses quietly instead of trapping the user.
  return ranCommand("detector_respond", () =>
    commands.detectorRespond(detectionId, action),
  );
}

const ToastOverlay: React.FC = () => {
  const { t } = useTranslation();
  const direction = getLanguageDirection(i18n.language);

  const [state, setState] = useState<ToastStateEvent>({
    collapsed: false,
    detection: null,
    notice: null,
  });
  const [hovered, setHovered] = useState(false);
  const [menuOpen, setMenuOpen] = useState(false);
  const [confirming, setConfirming] = useState<string | null>(null);

  const collapseTimerRef = useRef<number | undefined>(undefined);
  const confirmTimerRef = useRef<number | undefined>(undefined);
  const menuRef = useRef<HTMLDivElement>(null);
  const menuButtonRef = useRef<HTMLButtonElement>(null);
  // The card element — measured for the window height so a long notice
  // message grows the toast instead of clipping (the static per-view map
  // is the pre-mount fallback).
  const cardRef = useRef<HTMLDivElement>(null);
  // Last height reported to the native window — dedupes the per-frame
  // ResizeObserver callbacks the height transition produces.
  const lastHeightRef = useRef(0);

  const detection = state.detection;
  // The freshest detection inside awaited handlers — `respond` must not
  // act on a detection that a newer `toast://state` already replaced or
  // cleared (`null` mismatches the captured id the same way). Ref writes
  // live in an effect so render stays side-effect free (StrictMode).
  const detectionRef = useRef(detection);
  const noticeRef = useRef(state.notice);
  useEffect(() => {
    detectionRef.current = detection;
    noticeRef.current = state.notice;
  });
  // Set once per mount — awaited handlers check it before scheduling
  // timers or IPC that must not outlive the component. StrictMode's
  // mount → cleanup → mount cycle means the flag must be re-armed in
  // the effect body, not just cleared in the cleanup.
  const mountedRef = useRef(false);
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);
  // FR-008-13 (T-069): an `auto_start` detection is already recording —
  // resolve the "Gravando · <App>" face on the very first paint so the ask
  // prompt never flashes; the effect below only arms its 3 s dismissal.
  const autoConfirm =
    detection?.action === "auto_start" ? detection.app_label : null;
  const confirmingApp = confirming ?? autoConfirm;
  const view = resolveToastView({
    state,
    hovered,
    menuOpen,
    confirming: confirmingApp,
  });

  // ---- Collapse timer (FR-008-10) ------------------------------------------
  // 60 s without interaction hides the window; every pointer event on the
  // card re-arms it. The confirmation face is exempt — it ends itself.
  const disarmCollapse = useCallback(() => {
    window.clearTimeout(collapseTimerRef.current);
    collapseTimerRef.current = undefined;
  }, []);
  const armCollapse = useCallback(() => {
    window.clearTimeout(collapseTimerRef.current);
    collapseTimerRef.current = window.setTimeout(() => {
      void ranCommand("toast_set_collapsed", () =>
        commands.toastSetCollapsed(true),
      );
    }, COLLAPSE_AFTER_MS);
  }, []);

  useEffect(() => {
    // FR-009-09: the check-in stays up for its full response window —
    // collapsing it would hide the only way to say "keep recording".
    if (
      view === "hidden" ||
      view === "confirming" ||
      noticeBlocksCollapse(state.notice)
    ) {
      disarmCollapse();
      return;
    }
    armCollapse();
    return disarmCollapse;
  }, [view, state.notice, armCollapse, disarmCollapse]);

  // ---- toast://state --------------------------------------------------------
  useEffect(() => {
    void syncLanguageFromSettings();
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    listen<ToastStateEvent>("toast://state", (event) => {
      const payload = event.payload;
      // A *different* detection replaces the confirmation face and its
      // dismiss timer — a same-id re-emission must NOT clear them, or the
      // auto-start face (which only re-arms when `autoConfirm` changes)
      // would be left up forever with no way to dismiss. The check runs
      // before the collapsed early-return: a collapsed payload can still
      // carry a new detection that invalidates a pending confirmation.
      const detectionChanged =
        payload.detection?.detection_id !== detectionRef.current?.detection_id;
      if (detectionChanged) {
        setConfirming(null);
        window.clearTimeout(confirmTimerRef.current);
      }
      setState(payload);
      if (payload.collapsed) {
        setHovered(false);
        setMenuOpen(false);
        return;
      }
      setMenuOpen(false);
      void syncLanguageFromSettings();
    })
      .then((fn) => {
        // The effect may have been cleaned up while `listen` was in flight
        // (StrictMode double-mount) — a listener registered then would leak.
        if (cancelled) {
          fn();
        } else {
          unlisten = fn;
        }
      })
      .catch((e) => console.warn("toast://state listen failed:", e));
    return () => {
      cancelled = true;
      unlisten?.();
      disarmCollapse();
      window.clearTimeout(confirmTimerRef.current);
    };
  }, [disarmCollapse]);

  // ---- Window height follows the card (no dead click zone) ------------------
  // A ResizeObserver reports the card's border-box on every layout change,
  // including each frame of the height transition — measuring once right
  // after a view change would freeze the window at a mid-transition height
  // and leave the expanded actions outside the native window. The per-view
  // map is only the pre-mount fallback. Height is rounded up so fractional
  // CSS px (125 %/150 % display scaling) never clip the last row.
  useEffect(() => {
    let settleTimer: number | undefined;
    let retryTimer: number | undefined;
    let retries = 0;
    // A retry or in-flight invoke can land after cleanup — `disposed` stops
    // the timer chain and keeps post-unmount IPC/setState from happening.
    let disposed = false;
    const report = () => {
      if (disposed) return;
      // `offsetHeight` is pure layout — `getBoundingClientRect` would also
      // report the tcard-pop scale and the transition's intermediate sizes.
      const measured = cardRef.current?.offsetHeight;
      const height =
        measured && measured > 0
          ? measured + 32
          : toastWindowHeight(view, menuOpen);
      if (height <= 0) {
        // Hidden face: force the next visible report so a re-shown window
        // never inherits a stale dedupe value.
        lastHeightRef.current = 0;
        return;
      }
      if (height !== lastHeightRef.current) {
        lastHeightRef.current = height;
        void commands
          .toastSetContentHeight(height)
          .then((res) => {
            // A refused report (`{status:"error"}` resolves, it doesn't
            // reject) must take the same retry path as an IPC failure.
            if (res.status === "error") throw res.error;
            retries = 0;
          })
          .catch((e) => {
            // A rejected report must not wedge the dedupe — but only undo
            // the dedupe for THIS height; a newer report that landed
            // meanwhile keeps its value.
            if (lastHeightRef.current === height) {
              lastHeightRef.current = 0;
            }
            if (disposed) return;
            // Nothing re-triggers the observer after a settle (no layout
            // change), so retry here, bounded.
            if (retries < 4) {
              retries += 1;
              window.clearTimeout(retryTimer);
              retryTimer = window.setTimeout(report, 150);
            }
            console.warn("toast_set_content_height failed:", e);
          });
      }
    };
    const cleanup = () => {
      disposed = true;
      window.clearTimeout(settleTimer);
      window.clearTimeout(retryTimer);
    };
    report();
    const card = cardRef.current;
    if (!card || typeof ResizeObserver === "undefined") {
      return cleanup;
    }
    // The height transition fires the observer once per frame — coalesce
    // shrinking into one IPC per settle so every frame is not a native
    // window resize. Growth reports immediately: a debounce there would
    // clip expanding content for the whole transition.
    const schedule = () => {
      const measured = cardRef.current?.offsetHeight;
      const height =
        measured && measured > 0
          ? measured + 32
          : toastWindowHeight(view, menuOpen);
      if (height > lastHeightRef.current) {
        window.clearTimeout(settleTimer);
        report();
        return;
      }
      window.clearTimeout(settleTimer);
      settleTimer = window.setTimeout(report, 60);
    };
    const observer = new ResizeObserver(schedule);
    observer.observe(card);
    return () => {
      cleanup();
      observer.disconnect();
    };
    // The notice message length changes the wrapped height — re-measure on
    // every payload swap, not just on view changes.
  }, [view, menuOpen, state.notice?.message]);

  // ---- Actions --------------------------------------------------------------
  const dismissToast = useCallback(() => {
    void ranCommand("toast_dismiss", () => commands.toastDismiss());
  }, []);

  // Buttons act on pointerdown AND click, deduped *per gesture*: pointerdown
  // runs the action and stamps it consumed; the paired click is suppressed.
  // An unpaired click (keyboard, or a pointerdown the WebView2 focus
  // transition swallowed) still runs. The stamp is re-taken on pointerup —
  // the click lands at release, so a held press must measure freshness from
  // the release, not the press — and carries a freshness bound so a flag
  // whose paired click never arrived (press dragged off the button) can
  // only eat a gesture for a second. `pointerleave` never clears: a mouse
  // press that leaves, re-enters and releases still pairs with a click,
  // and a touch/pen release fires leave *before* the click — clearing on
  // either would double-run. `pointercancel` clears because a cancelled
  // press produces no click at all. (Same pattern as the assistant
  // panel's stripButton.)
  const consumedGestureRef = useRef<Partial<Record<string, number>>>({});
  const pressable = (id: string, action: () => void) => {
    const clear = () => {
      delete consumedGestureRef.current[id];
    };
    const restamp = () => {
      if (consumedGestureRef.current[id] !== undefined) {
        consumedGestureRef.current[id] = performance.now();
      }
    };
    return {
      onPointerDown: (e: React.PointerEvent) => {
        if (e.button !== 0) return; // only the primary button activates
        consumedGestureRef.current[id] = performance.now();
        action();
      },
      onPointerUp: restamp,
      onPointerCancel: clear,
      onClick: () => {
        const stamp = consumedGestureRef.current[id];
        clear();
        if (stamp !== undefined && performance.now() - stamp < 1000) {
          return; // the click paired with the pointerdown that already ran
        }
        action();
      },
    };
  };

  // FR-008-14/FR-009-08/09/22 (T-069): a notice carrying a known action
  // gets button(s); success dismisses, a refused/failed command keeps the
  // notice so retry stays reachable.
  const runNoticeAction = useCallback(
    async (action: NoticeAction, button: NoticeActionButton = action) => {
      const notice = state.notice;
      const meetingId = notice?.meeting_id ?? null;
      const args: Record<string, unknown> = { ...button.args };
      if (action.needsMeetingId) {
        if (!meetingId) return; // no context → nothing safe to invoke
        args.meetingId = meetingId;
      }
      // Raw invoke: resolve means the command ran (the payload is whatever
      // the backend returned), reject means it refused or failed.
      let ok = true;
      try {
        const res = await invoke<unknown>(button.command, args);
        // FR-009-02 "Copiar aviso": the command resolves to the reminder
        // text — the dismissal only counts once it's on the clipboard.
        if (action.copiesTextToClipboard) {
          ok = typeof res === "string";
          if (ok && typeof res === "string") {
            await navigator.clipboard.writeText(res);
          }
        }
      } catch (e) {
        console.warn(`${button.command} failed:`, e);
        ok = false;
      }
      if (!ok) return;
      if (!mountedRef.current) return;
      // The command already ran — navigation follows it regardless. Only
      // the dismissal is gated on staleness: a *different* notice (or a
      // detection) that arrived mid-await must not be dismissed by this
      // action's success. Notices compare by content — each toast://state
      // payload deserializes to a fresh object.
      if (action.navigateSection) {
        emit("hub://navigate", {
          section: action.navigateSection,
          settingsTab: action.navigateSettingsTab,
        }).catch((e) => console.warn("hub://navigate emit failed:", e));
      }
      const current = noticeRef.current;
      const sameNotice =
        current !== null &&
        notice !== null &&
        current.kind === notice.kind &&
        current.message === notice.message &&
        current.action === notice.action &&
        current.meeting_id === notice.meeting_id;
      if (sameNotice) dismissToast();
    },
    [state.notice, dismissToast],
  );

  const dismissTransient = useCallback(() => {
    setMenuOpen(false);
    setHovered(false);
  }, []);

  // Menu keyboard behavior matching the `menu`/`menuitem` roles: opening
  // moves focus to the first item, arrows cycle, Escape closes and returns
  // focus to the trigger.
  useEffect(() => {
    if (menuOpen) {
      menuRef.current?.querySelector<HTMLElement>('[role="menuitem"]')?.focus();
    }
  }, [menuOpen]);
  const onMenuKeyDown = (e: React.KeyboardEvent) => {
    if (e.altKey || e.ctrlKey || e.metaKey) return; // plain keys only
    const items = Array.from(
      menuRef.current?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? [],
    );
    const active = document.activeElement;
    const at = active instanceof HTMLElement ? items.indexOf(active) : -1;
    if (e.key === "Escape") {
      e.preventDefault();
      setMenuOpen(false);
      menuButtonRef.current?.focus();
    } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      // Focus may still sit on the trigger (mouse-opened menu on an
      // unfocused webview): -1 must enter at the edge, not skip an item.
      const next =
        at < 0
          ? e.key === "ArrowDown"
            ? 0
            : items.length - 1
          : (at + (e.key === "ArrowDown" ? 1 : -1) + items.length) %
            items.length;
      items[next]?.focus();
    } else if (e.key === "Home") {
      e.preventDefault();
      items[0]?.focus();
    } else if (e.key === "End") {
      e.preventDefault();
      items[items.length - 1]?.focus();
    } else if (e.key === "Tab") {
      // A menu is a transient surface — Tab closes it and parks focus on
      // the trigger: without this the focused menuitem unmounts and focus
      // drops to the document.
      e.preventDefault();
      setMenuOpen(false);
      menuButtonRef.current?.focus();
    }
  };

  const respond = useCallback(
    async (action: DetectorAction) => {
      const det = detection;
      dismissTransient();
      if (!det) {
        dismissToast();
        return;
      }
      const ok = await detectorRespond(det.detection_id, action);
      // The command awaited — a newer `toast://state` may have replaced or
      // cleared this detection meanwhile; its confirmation must not claim
      // the old app label, arm a dismiss on a newer toast, or fire IPC
      // after the component is gone.
      if (!mountedRef.current) return;
      if (detectionRef.current?.detection_id !== det.detection_id) return;
      if (ok && startsRecording(action)) {
        // FR-008-09: "Gravando · <App>" for 3 s, then the window goes away.
        setConfirming(det.app_label);
        window.clearTimeout(confirmTimerRef.current);
        confirmTimerRef.current = window.setTimeout(
          dismissToast,
          CONFIRMATION_MS,
        );
      } else {
        dismissToast();
      }
    },
    [detection, dismissToast, dismissTransient],
  );

  // FR-008-13: the auto-start confirmation self-dismisses after 3 s (the
  // face itself is resolved at paint time via `autoConfirm` above). Keyed
  // on the detection id, not just the label — a new `auto_start` for the
  // same app changes `detection_id` but not `autoConfirm`, and the
  // listener has already cleared the old timer, so the effect must
  // re-arm or the face sticks forever.
  useEffect(() => {
    if (autoConfirm === null) {
      return;
    }
    window.clearTimeout(confirmTimerRef.current);
    confirmTimerRef.current = window.setTimeout(dismissToast, CONFIRMATION_MS);
  }, [autoConfirm, detection?.detection_id, dismissToast]);

  if (view === "hidden") {
    // Keep the live region mounted — a region inserted together with its
    // content is often not announced.
    return (
      <div dir={direction} className="toast-stage">
        <span className="toast-sr" aria-live="polite" />
      </div>
    );
  }

  // ---- Render ---------------------------------------------------------------

  const translatedError = state.notice?.kind === "translated_dictation_error";
  const noticeMessage = translatedError
    ? t(
        `settings.translatedDictation.errors.${TRANSLATION_ERROR_CODES.has(state.notice?.message ?? "") ? state.notice?.message : "translation_failed"}`,
      )
    : state.notice?.message;

  const renderNotice = () => {
    const expanded = view === "notice-expanded";
    const noticeAction = translatedError
      ? null
      : noticeActionFor(state.notice?.action, state.notice?.meeting_id);
    // The countdown restarts per notice payload (keyed on kind+message);
    // hover pauses it, the bar's own animationend dismisses. Check-in
    // notices keep their 2-min answer window — no bar, no auto-dismiss.
    const selfDismiss = noticeSelfDismisses(state.notice);
    return (
      <div
        ref={cardRef}
        className={`tcard tnotice ${expanded && noticeAction ? "expanded" : ""}`}
        onMouseEnter={() => setHovered(true)}
        onMouseLeave={() => setHovered(false)}
        onPointerMove={
          noticeBlocksCollapse(state.notice) ? undefined : armCollapse
        }
      >
        <div className="tnotice-main">
          {isWarningKind(state.notice?.kind) ? (
            <TriangleAlert
              size={16}
              className="ticon-warn"
              aria-hidden="true"
            />
          ) : (
            <BellRing size={16} className="ticon" aria-hidden="true" />
          )}
          <div
            className={`tnotice-msg ${translatedError ? "tnotice-translation" : ""}`}
          >
            {translatedError && (
              <h2 className="tnotice-title">
                {t("settings.translatedDictation.errorTitle")}
              </h2>
            )}
            <span className="tnotice-body">{noticeMessage}</span>
          </div>
          {expanded && (
            <button
              type="button"
              className="sx tclose"
              aria-label={t("toast.dismiss")}
              {...pressable("notice-close", dismissToast)}
            >
              <X size={10} aria-hidden="true" />
            </button>
          )}
        </div>
        {expanded && noticeAction && (
          <div className="tactions">
            <button
              type="button"
              className="tprimary"
              {...pressable(
                "notice-action",
                () => void runNoticeAction(noticeAction),
              )}
            >
              {t(`toast.${noticeAction.labelKey}`)}
            </button>
            {noticeAction.secondary && (
              <button
                type="button"
                className="tsecondary"
                {...pressable(
                  "notice-secondary",
                  () =>
                    void runNoticeAction(noticeAction, noticeAction.secondary),
                )}
              >
                {t(`toast.${noticeAction.secondary.labelKey}`)}
              </button>
            )}
          </div>
        )}
        {selfDismiss && (
          <div
            key={`${state.notice?.kind}:${state.notice?.message}`}
            className="tnotice-life"
            style={{
              animationDuration: `${NOTICE_AUTO_DISMISS_MS}ms`,
              animationPlayState: hovered ? "paused" : "running",
            }}
            onAnimationEnd={dismissToast}
            aria-hidden="true"
          />
        )}
      </div>
    );
  };

  const renderMeeting = () => {
    if (view === "confirming") {
      return (
        <div ref={cardRef} className="tcard tconfirm">
          <span className="tdot-rec" aria-hidden="true" />
          <span
            className="ttitle"
            title={t("toast.recording", { app: confirmingApp })}
          >
            {t("toast.recording", { app: confirmingApp })}
          </span>
        </div>
      );
    }
    return (
      <MeetingCard
        detection={detection}
        menuOpen={menuOpen}
        cardRef={cardRef}
        menuRef={menuRef}
        menuButtonRef={menuButtonRef}
        pressable={pressable}
        onRespond={(action) => void respond(action)}
        onToggleMenu={() => setMenuOpen((v) => !v)}
        onHoverChange={(isHovered) => {
          setHovered(isHovered);
          if (!isHovered) setMenuOpen(false);
        }}
        onPointerMove={armCollapse}
        onMenuKeyDown={onMenuKeyDown}
      />
    );
  };

  return (
    <div dir={direction} className="toast-stage">
      <span className="toast-sr" aria-live="polite">
        {view === "confirming"
          ? t("toast.recording", { app: confirmingApp })
          : detection
            ? t("toast.meetingDetected")
            : (noticeMessage ?? "")}
      </span>
      {state.notice !== null && detection === null
        ? renderNotice()
        : renderMeeting()}
    </div>
  );
};

export default ToastOverlay;
