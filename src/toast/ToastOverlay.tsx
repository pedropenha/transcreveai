import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import React, { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { BellRing, ChevronDown, TriangleAlert, Video, X } from "lucide-react";
import "./ToastOverlay.css";
import i18n, { syncLanguageFromSettings } from "@/i18n";
import { getLanguageDirection } from "@/lib/utils/rtl";
import {
  COLLAPSE_AFTER_MS,
  CONFIRMATION_MS,
  NOTICE_AUTO_DISMISS_MS,
  meetingMenuItems,
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
 * The meeting toast (F008 / T-062): compact card that expands on hover.
 * Everything user-facing comes from `toast://state`; the 60 s collapse timer
 * (FR-008-10) lives here — on expiry it reports `toast_set_collapsed` and the
 * backend hides the window + lights the Flow Bar's amber dot.
 */

/** Envelope a tauri-specta `CommandResult` command resolves to. Raw `invoke`
 *  (not `commands.*`) because `detector_respond` is T-061's and not yet in
 *  bindings.ts. */
interface CommandEnvelope {
  status: "ok" | "error";
  data?: unknown;
  error?: unknown;
}

/** Kinds that render the warning triangle rather than the bell —
 *  `toast://show.kind` is a semantic discriminator, not a style. */
function isWarningKind(kind: string | undefined): boolean {
  return (
    kind === "warning" ||
    kind === "error" ||
    kind === "meeting_error" ||
    kind === "meeting_warning" ||
    kind === "meeting_limit" ||
    kind === "meeting_auto_stop"
  );
}

async function detectorRespond(
  detectionId: string,
  action: DetectorAction,
): Promise<boolean> {
  try {
    const res = await invoke<CommandEnvelope | null>("detector_respond", {
      detectionId,
      action,
    });
    // tauri-specta commands resolve with the {status} envelope; a bare
    // Result<()> backend (without specta) resolves null — count it as ok.
    if (res !== null && typeof res === "object" && "status" in res) {
      return res.status === "ok";
    }
    return true;
  } catch (e) {
    // Until T-061 lands the command doesn't exist — treat like any backend
    // refusal: the toast dismisses quietly instead of trapping the user.
    console.warn("detector_respond failed:", e);
    return false;
  }
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
  // The card element — measured for the window height so a long notice
  // message grows the toast instead of clipping (the static per-view map
  // is the pre-mount fallback).
  const cardRef = useRef<HTMLDivElement>(null);

  const detection = state.detection;
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
      void invoke("toast_set_collapsed", { collapsed: true });
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
    listen<ToastStateEvent>("toast://state", (event) => {
      const payload = event.payload;
      setState(payload);
      if (payload.collapsed) {
        setHovered(false);
        setMenuOpen(false);
        return;
      }
      // Fresh presentation or reopen: close the menu; a *new* detection also
      // clears any in-flight confirmation from the replaced one.
      setMenuOpen(false);
      setConfirming(null);
      void syncLanguageFromSettings();
    }).then((fn) => {
      unlisten = fn;
    });
    return () => {
      unlisten?.();
      disarmCollapse();
      window.clearTimeout(confirmTimerRef.current);
    };
  }, [disarmCollapse]);

  // ---- Window height follows the card (no dead click zone) ------------------
  // Prefer the measured card height (a wrapped notice message can exceed
  // the static 104 px row); the per-view map is the fallback before mount.
  useEffect(() => {
    const measured = cardRef.current?.getBoundingClientRect().height;
    const height =
      measured && measured > 0
        ? measured + 32
        : toastWindowHeight(view, menuOpen);
    if (height > 0) {
      void invoke("toast_set_content_height", { height });
    }
    // The notice message length changes the wrapped height — re-measure on
    // every payload swap, not just on view changes.
  }, [view, menuOpen, state.notice?.message]);

  // ---- Actions --------------------------------------------------------------
  const dismissToast = useCallback(() => {
    void invoke("toast_dismiss");
  }, []);

  // Buttons act on pointerdown AND click, deduped per target — whichever
  // event survives the WebView2 focus transition wins; the second is
  // ignored within a 400 ms window. (Same pattern as the assistant panel's
  // stripButton.)
  const lastPressRef = useRef<Record<string, number>>({});
  const pressable = (id: string, action: () => void) => {
    const run = () => {
      const now = Date.now();
      if (now - (lastPressRef.current[id] ?? 0) < 400) return;
      lastPressRef.current[id] = now;
      action();
    };
    return {
      onPointerDown: () => run(),
      onClick: () => run(),
    };
  };

  // FR-008-14/FR-009-08/09/22 (T-069): a notice carrying a known action
  // gets button(s); success dismisses, a refused/failed command keeps the
  // notice so retry stays reachable.
  const runNoticeAction = useCallback(
    async (action: NoticeAction, button: NoticeActionButton = action) => {
      const meetingId = state.notice?.meeting_id ?? null;
      const args: Record<string, unknown> = { ...button.args };
      if (action.needsMeetingId) {
        if (!meetingId) return; // no context → nothing safe to invoke
        args.meetingId = meetingId;
      }
      let ok = false;
      try {
        const res = await invoke<CommandEnvelope | null>(button.command, args);
        ok = !(
          res !== null &&
          typeof res === "object" &&
          res.status === "error"
        );
        // FR-009-02 "Copiar aviso": the command resolves to the reminder
        // text — the dismissal only counts once it's on the clipboard.
        if (ok && action.copiesTextToClipboard) {
          const text =
            res !== null && typeof res === "object" && "data" in res
              ? res.data
              : res;
          ok = typeof text === "string";
          if (ok) await navigator.clipboard.writeText(text as string);
        }
      } catch (e) {
        console.warn(`${button.command} failed:`, e);
        ok = false;
      }
      if (!ok) return;
      if (action.navigateSection) {
        void emit("hub://navigate", {
          section: action.navigateSection,
          settingsTab: action.navigateSettingsTab,
        });
      }
      dismissToast();
    },
    [state.notice, dismissToast],
  );

  const dismissTransient = useCallback(() => {
    setMenuOpen(false);
    setHovered(false);
  }, []);

  const respond = useCallback(
    async (action: DetectorAction) => {
      const det = detection;
      dismissTransient();
      if (!det) {
        dismissToast();
        return;
      }
      const ok = await detectorRespond(det.detection_id, action);
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
  // face itself is resolved at paint time via `autoConfirm` above).
  useEffect(() => {
    if (autoConfirm === null) {
      return;
    }
    window.clearTimeout(confirmTimerRef.current);
    confirmTimerRef.current = window.setTimeout(dismissToast, CONFIRMATION_MS);
  }, [autoConfirm, dismissToast]);

  if (view === "hidden") return null;

  // ---- Render ---------------------------------------------------------------

  const renderNotice = () => {
    const expanded = view === "notice-expanded";
    const noticeAction = noticeActionFor(
      state.notice?.action,
      state.notice?.meeting_id,
    );
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
        onPointerMove={armCollapse}
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
          <span className="tnotice-msg">{state.notice?.message}</span>
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
          <span className="ttitle">
            {t("toast.recording", { app: confirmingApp })}
          </span>
        </div>
      );
    }
    const expanded = view === "expanded";
    return (
      <div
        ref={cardRef}
        className={`tcard ${expanded ? "expanded" : ""}`}
        onMouseEnter={() => setHovered(true)}
        onMouseLeave={() => {
          setHovered(false);
          setMenuOpen(false);
        }}
        onPointerMove={armCollapse}
      >
        <div className="trow">
          <Video size={16} className="ticon" aria-hidden="true" />
          <div className="ttexts">
            <span className="ttitle">{t("toast.meetingDetected")}</span>
            <span className="tsub">
              <i className="tnow" aria-hidden="true" />
              {t("toast.nowWithApp", { app: detection?.app_label })}
            </span>
          </div>
          {expanded && (
            <button
              type="button"
              className="sx tclose"
              aria-label={t("toast.dismiss")}
              {...pressable("meeting-close", () => void respond("dismiss"))}
            >
              <X size={10} aria-hidden="true" />
            </button>
          )}
        </div>
        {expanded && (
          <div className="tactions">
            <button
              type="button"
              className="tprimary"
              {...pressable("meeting-start", () => void respond("start"))}
            >
              {t("toast.startNotetaker")}
            </button>
            <button
              type="button"
              className="tmenu-btn"
              aria-label={t("toast.moreActions")}
              aria-expanded={menuOpen}
              aria-haspopup="menu"
              {...pressable("meeting-menu", () => setMenuOpen((v) => !v))}
            >
              <ChevronDown size={12} aria-hidden="true" />
            </button>
          </div>
        )}
        {expanded && menuOpen && (
          <div className="tmenu" role="menu">
            {meetingMenuItems().map((item) => (
              <button
                key={item.action}
                type="button"
                role="menuitem"
                className="tmenu-item"
                {...pressable(
                  `menu-${item.action}`,
                  () => void respond(item.action),
                )}
              >
                {t(`toast.${item.labelKey}`, {
                  app: detection?.app_label,
                })}
              </button>
            ))}
          </div>
        )}
      </div>
    );
  };

  return (
    <div dir={direction} className="toast-stage">
      <span className="toast-sr" aria-live="polite">
        {view === "confirming"
          ? t("toast.recording", { app: confirming })
          : detection
            ? t("toast.meetingDetected")
            : (state.notice?.message ?? "")}
      </span>
      {state.notice !== null && detection === null
        ? renderNotice()
        : renderMeeting()}
    </div>
  );
};

export default ToastOverlay;
