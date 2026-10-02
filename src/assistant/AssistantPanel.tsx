import React, { useCallback, useEffect, useRef, useState } from "react";
import { emit, listen } from "@tauri-apps/api/event";
import ReactMarkdown from "react-markdown";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useTranslation } from "react-i18next";
import { MessageSquarePlus, Pin, PinOff, Sparkles, X } from "lucide-react";
import "./AssistantPanel.css";
import { commands, events } from "@/bindings";
import type { AssistantStateEvent, StreamTextEvent } from "@/bindings";
import i18n from "@/i18n";
import { getLanguageDirection } from "@/lib/utils/rtl";
import {
  canDragPanel,
  errorKindKey,
  isPanelDragging,
  pinToggleKey,
  providerHintIsAdvisory,
  providerHintKey,
  safeMarkdownUrl,
  type PanelGrab,
} from "./assistantView";

/** How long the last live-preview fragment stays visible after dictation
 * stops — the auto-sent user message normally replaces it almost
 * immediately; this is only the fallback for a lost trailing event. */
const LIVE_SETTLE_MS = 1500;

const AssistantPanel: React.FC = () => {
  const { t } = useTranslation();
  const direction = getLanguageDirection(i18n.language);

  const [state, setState] = useState<AssistantStateEvent | null>(null);
  // Live STT preview while a dictation is routed to the panel — rendered as
  // a pending user bubble until the finalized transcript lands as a real
  // message (FR-012-12: the visible transcription the user approved).
  const [live, setLive] = useState("");
  const scrollRef = useRef<HTMLDivElement>(null);
  // Title-strip drag (T-092): the grab offset captured at pointerdown plus
  // an rAF ticket so moves land at most once per frame — the store is only
  // written once, on pointerup (`assistant_save_panel_position`).
  const grabRef = useRef<PanelGrab | null>(null);
  const dragFrameRef = useRef(0);
  // The view-model decisions read the freshest state inside event handlers.
  const stateRef = useRef<AssistantStateEvent | null>(null);
  stateRef.current = state;

  useEffect(() => {
    let unlisteners: (() => void)[] = [];
    let cancelled = false;

    const setup = async () => {
      // Hydrate before wiring listeners so a cold webview renders the full
      // snapshot (open conversation, pinned state, provider chip).
      try {
        const result = await commands.assistantGetState();
        if (cancelled) return;
        if (result.status === "ok") {
          setState(result.data);
          stateRef.current = result.data;
        }
      } catch {
        // The panel can still render its chrome without a snapshot; the next
        // `assistant://state` event recovers it.
      }

      // Each listener self-unlistens if the effect was torn down while its
      // `listen` promise was still in flight — otherwise the callbacks keep
      // firing on a dead component for the rest of the session.
      const add = async (
        register: () => Promise<() => void>,
      ): Promise<void> => {
        const unlisten = await register();
        if (cancelled) unlisten();
        else unlisteners.push(unlisten);
      };

      await add(() =>
        listen<AssistantStateEvent>("assistant://state", (event) => {
          setState(event.payload);
        }),
      );

      // The streaming engine broadcasts committed+tentative text to every
      // window; the panel only shows it while this dictation is routed here.
      await add(() =>
        events.streamTextEvent.listen((event) => {
          if (stateRef.current?.dictating) {
            const p = event.payload as StreamTextEvent;
            setLive(`${p.committed}${p.tentative ? ` ${p.tentative}` : ""}`);
          }
        }),
      );
    };

    void setup();
    return () => {
      cancelled = true;
      if (dragFrameRef.current !== 0) {
        cancelAnimationFrame(dragFrameRef.current);
        dragFrameRef.current = 0;
      }
      unlisteners.forEach((fn) => fn());
    };
    // Mount-once: handlers read via refs, so no listener ever goes stale.
  }, []);

  // The live bubble is replaced by the finalized user message the moment
  // the pipeline auto-sends it — clear the preview as soon as a user
  // message lands (or, as a fallback, a beat after dictation stops).
  const messageCount = state?.messages.length ?? 0;
  useEffect(() => {
    if (state?.dictating !== false) return;
    if (!live) return;
    const lastUser = [...(state?.messages ?? [])]
      .reverse()
      .find((m) => m.role === "user");
    if (lastUser && lastUser.content.trim() === live.trim()) {
      setLive("");
      return;
    }
    const id = window.setTimeout(() => setLive(""), LIVE_SETTLE_MS);
    return () => window.clearTimeout(id);
  }, [state?.dictating, state?.messages, live, messageCount]);

  // Keep the newest turn in view as messages/thinking land.
  useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [state?.messages, state?.phase, state?.queuedPrompt, live]);

  // ---- Actions ------------------------------------------------------------

  // Esc anywhere in the panel: cancel while thinking, close otherwise
  // (AC-012-05 — the backend aborts the provider future).
  const onStageKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      void commands.assistantDismiss();
    }
  };

  // FR-012-11: the panel never takes focus on its own — a click is explicit
  // intent, so the backend makes the window key/focused here.
  const onPanelMouseDown = () => {
    void commands.assistantFocus();
  };

  // ---- Drag & pin (T-092, FR-012-16) ------------------------------------
  // Pointer-based drag → commands: each move reports only the grab offset
  // (clientX/Y at pointerdown — constant while the window tracks the
  // cursor); the backend reads the cursor itself in physical px, because a
  // webview screenX/Y is DIP and ambiguous on mixed-DPI layouts.
  // Persistence happens once on drag end, not at move rate.

  const onTitlePointerDown = (e: React.PointerEvent<HTMLElement>) => {
    if (e.button !== 0) return;
    if (!canDragPanel(stateRef.current?.pinned ?? false)) return;
    // The buttons inside the strip are not drag handles.
    if ((e.target as HTMLElement).closest("button")) return;
    grabRef.current = { clientX: e.clientX, clientY: e.clientY };
    e.currentTarget.setPointerCapture(e.pointerId);
  };

  const onTitlePointerMove = () => {
    const grab = grabRef.current;
    if (!isPanelDragging(grab) || !grab) return;
    if (dragFrameRef.current !== 0) return;
    const { clientX, clientY } = grab;
    dragFrameRef.current = requestAnimationFrame(() => {
      dragFrameRef.current = 0;
      void commands.assistantMovePanel(clientX, clientY);
    });
  };

  const endTitleDrag = () => {
    const grab = grabRef.current;
    if (!isPanelDragging(grab) || !grab) return;
    grabRef.current = null;
    if (dragFrameRef.current !== 0) {
      cancelAnimationFrame(dragFrameRef.current);
      dragFrameRef.current = 0;
    }
    void commands.assistantSavePanelPosition(grab.clientX, grab.clientY);
  };

  // FR-012-16 "Fixar": the toggle persists; while pinned every drag is
  // ignored (canDragPanel gate above + a backend check on the command).
  const togglePin = () => {
    void commands.assistantSetPanelPinned(!(stateRef.current?.pinned ?? false));
  };

  const close = useCallback(() => void commands.assistantClose(), []);
  const cancel = () => void commands.assistantCancel();
  const retry = () => void commands.assistantRetry();
  const newConversation = () => void commands.assistantNewConversation();
  const openSettings = () => {
    void commands.showMainWindowCommand();
    // Lands on the Settings section's general tab, where the assistant
    // provider picker lives (`settingsTab` is read by SettingsHub).
    void emit("hub://navigate", {
      section: "settings",
      settingsTab: "general",
    });
  };

  /** Title-strip buttons act on pointerdown AND click, deduped per button —
   * whichever event survives the WebView2 focus transition wins; the
   * follow-up within 400 ms is ignored (a plain click alone can be
   * swallowed by the native focus transition — the X "not closing" bug). */
  const lastStripPressRef = useRef<Record<string, number>>({});
  const stripButton = (id: string, action: () => void) => {
    const run = () => {
      const now = Date.now();
      if (now - (lastStripPressRef.current[id] ?? 0) < 400) return;
      lastStripPressRef.current[id] = now;
      action();
    };
    return {
      onPointerDown: (e: React.PointerEvent) => {
        e.preventDefault();
        e.stopPropagation();
        run();
      },
      onMouseDown: (e: React.MouseEvent) => e.stopPropagation(),
      onClick: (e: React.MouseEvent) => {
        e.stopPropagation();
        run();
      },
    };
  };

  // ---- Render -------------------------------------------------------------

  const dictating = state?.dictating ?? false;
  const pinned = state?.pinned ?? false;
  const phase = state?.phase ?? "idle";
  const messages = state?.messages ?? [];
  const queued = state?.queuedPrompt ?? null;
  const experimental = providerHintIsAdvisory(state?.providerHint);
  const showEmpty =
    messages.length === 0 && phase === "idle" && !dictating && state !== null;

  return (
    <div
      className="as-stage"
      dir={direction}
      onMouseDown={onPanelMouseDown}
      onKeyDown={onStageKeyDown}
    >
      <div className="as-panel" role="dialog" aria-label={t("assistant.title")}>
        {/* Title strip — T-092's drag handle + Fixar toggle (FR-012-16):
            pointer events on the strip move the window; the buttons are
            excluded from the drag so they still click. */}
        <header
          className={`as-title${pinned ? " as-title--pinned" : ""}`}
          onPointerDown={onTitlePointerDown}
          onPointerMove={onTitlePointerMove}
          onPointerUp={endTitleDrag}
          onPointerCancel={endTitleDrag}
        >
          <Sparkles size={13} className="as-title-icon" aria-hidden="true" />
          <span className="as-title-text">{t("assistant.title")}</span>
          {state?.providerLabel && (
            <button
              type="button"
              className={`as-provider${experimental ? " as-provider--experimental" : ""}`}
              title={state.providerId ?? undefined}
              aria-label={t("assistant.changeProvider")}
              {...stripButton("provider", openSettings)}
            >
              {state.providerLabel}
              {experimental && (
                <em className="as-exp"> {t("assistant.experimental")}</em>
              )}
            </button>
          )}
          <span className="as-title-spacer" />
          <button
            type="button"
            className="as-pin"
            aria-label={t("assistant.newConversation")}
            title={t("assistant.newConversation")}
            {...stripButton("new", newConversation)}
          >
            <MessageSquarePlus size={12} aria-hidden="true" />
          </button>
          <button
            type="button"
            className={`as-pin${pinned ? " as-pin--on" : ""}`}
            aria-label={t(pinToggleKey(pinned))}
            aria-pressed={pinned}
            title={t(pinToggleKey(pinned))}
            {...stripButton("pin", togglePin)}
          >
            {pinned ? (
              <PinOff size={12} aria-hidden="true" />
            ) : (
              <Pin size={12} aria-hidden="true" />
            )}
          </button>
          <button
            type="button"
            className="as-x"
            aria-label={t("assistant.close")}
            {...stripButton("close", close)}
          >
            <X size={12} aria-hidden="true" />
          </button>
        </header>

        {state && !state.providerReady && (
          <div className="as-banner" role="status">
            <p className="as-banner-text">
              <strong>{t("assistant.noProvider")}</strong>{" "}
              {t(providerHintKey(state.providerHint))}
            </p>
            <button
              type="button"
              className="as-banner-cta"
              onClick={openSettings}
            >
              {t("assistant.openSettings")}
            </button>
          </div>
        )}

        <div className="as-scroll" ref={scrollRef}>
          {showEmpty && (
            <div className="as-empty">
              <p>{t("assistant.empty")}</p>
              <p className="as-privacy">{t("assistant.privacyHint")}</p>
            </div>
          )}
          {messages.map((m, i) => (
            <div key={i} className={`as-msg as-${m.role}`}>
              {m.role === "assistant" ? (
                <div className="as-md">
                  <ReactMarkdown
                    skipHtml
                    // Model output is untrusted: no images (an <img> would
                    // silently fetch an arbitrary URL — tracking/beacon
                    // surface), and links limited to http/https/mailto by
                    // the url transform before they ever reach openUrl.
                    disallowedElements={["img"]}
                    urlTransform={safeMarkdownUrl}
                    components={{
                      a: ({ children, href }) => (
                        <a
                          href={href}
                          onClick={(e) => {
                            e.preventDefault();
                            if (href) void openUrl(href);
                          }}
                        >
                          {children}
                        </a>
                      ),
                    }}
                  >
                    {m.content}
                  </ReactMarkdown>
                </div>
              ) : (
                m.content
              )}
            </div>
          ))}
          {queued && (
            <div className="as-msg as-user as-queued" role="status">
              {queued}
              <span className="as-queued-tag">{t("assistant.queued")}</span>
            </div>
          )}
          {dictating && (
            <div className="as-msg as-user as-live-msg" role="status">
              <span className="as-live">
                <span className="as-dot" />
                {t("assistant.listening")}
              </span>
              {live && <span className="as-live-text">{live}</span>}
            </div>
          )}
          {phase === "thinking" && (
            <div className="as-msg as-assistant as-thinking" role="status">
              <span className="as-spinner" />
              <span className="as-thinking-text">
                {state?.providerLabel
                  ? t("assistant.thinkingVia", {
                      provider: state.providerLabel,
                    })
                  : t("assistant.thinking")}
              </span>
              <button type="button" className="as-cancel" onClick={cancel}>
                {t("assistant.cancel")}
              </button>
            </div>
          )}
          {phase === "error" && (
            <div className="as-msg as-error" role="alert">
              <span className="as-error-text">
                {/* Primary line is localized via errorKind — errorDetail
                    (raw backend text) stays as secondary diagnostics. */}
                {t(errorKindKey(state?.errorKind))}
                {state?.errorDetail && (
                  <span className="as-error-detail">{state.errorDetail}</span>
                )}
              </span>
              <button type="button" className="as-retry" onClick={retry}>
                {t("assistant.retry")}
              </button>
            </div>
          )}
          {phase === "cancelled" && (
            <div className="as-msg as-cancelled" role="status">
              {t("assistant.cancelled")}
              <button type="button" className="as-retry" onClick={retry}>
                {t("assistant.retry")}
              </button>
            </div>
          )}
        </div>

        <footer className="as-foot">
          <span className="as-hint">{t("assistant.dictateHint")}</span>
        </footer>
      </div>
    </div>
  );
};

export default AssistantPanel;
