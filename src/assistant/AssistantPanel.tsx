import React, { useCallback, useEffect, useRef, useState } from "react";
import { emit, listen } from "@tauri-apps/api/event";
import ReactMarkdown from "react-markdown";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useTranslation } from "react-i18next";
import { Pin, PinOff, Sparkles, X } from "lucide-react";
import "./AssistantPanel.css";
import { commands, events } from "@/bindings";
import type { AssistantStateEvent, StreamTextEvent } from "@/bindings";
import i18n from "@/i18n";
import { getLanguageDirection } from "@/lib/utils/rtl";
import {
  appendDictated,
  canDragPanel,
  canSend,
  composerValue,
  errorKindKey,
  hotkeyIntent,
  isPanelDragging,
  pinToggleKey,
  providerHintKey,
  safeMarkdownUrl,
  type AssistantViewState,
  type PanelGrab,
} from "./assistantView";

/** `assistant://dictated` payload — the finalized text of a routed
 * dictation, appended to the editable prompt (FR-012-12). */
interface DictatedPayload {
  text: string;
}

/** How long the last live-preview fragment stays visible after the stream
 * stops while waiting for the finalized `assistant://dictated` text. */
const DICTATED_SETTLE_MS = 3000;

const AssistantPanel: React.FC = () => {
  const { t } = useTranslation();
  const direction = getLanguageDirection(i18n.language);

  const [state, setState] = useState<AssistantStateEvent | null>(null);
  const [draft, setDraft] = useState("");
  // Live STT preview routed to the panel — shown inside the composer while
  // `state.dictating`, folded into `draft` on `assistant://dictated`.
  const [live, setLive] = useState("");
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  // Title-strip drag (T-092): the grab offset captured at pointerdown plus
  // an rAF ticket so moves land at most once per frame — the store is only
  // written once, on pointerup (`assistant_save_panel_position`).
  const grabRef = useRef<PanelGrab | null>(null);
  const dragFrameRef = useRef(0);
  // The view-model decisions read the freshest state inside event handlers.
  const stateRef = useRef<AssistantStateEvent | null>(null);
  stateRef.current = state;
  const draftRef = useRef(draft);
  draftRef.current = draft;

  const focusInput = useCallback(() => {
    inputRef.current?.focus();
  }, []);

  const send = useCallback((text: string) => {
    const s = stateRef.current;
    if (!s) return;
    if (!canSend(text, { phase: s.phase, providerReady: s.providerReady })) {
      return;
    }
    void commands.assistantSend(text).then((result) => {
      if (result.status === "ok") {
        setDraft("");
        setLive("");
      }
    });
  }, []);

  /** Second hotkey press with the panel open (FR-012-13): send a non-empty
   * draft, otherwise pull keyboard focus into the input. */
  const handleHotkey = useCallback(() => {
    const s = stateRef.current;
    if (!s) return;
    const intent = hotkeyIntent(draftRef.current, {
      phase: s.phase,
      providerReady: s.providerReady,
    });
    if (intent === "send") {
      send(draftRef.current);
    } else if (intent === "focus") {
      void commands.assistantFocus();
      focusInput();
    }
  }, [send, focusInput]);

  useEffect(() => {
    let unlisteners: (() => void)[] = [];
    let cancelled = false;

    const setup = async () => {
      // Hydrate before wiring listeners: `assistant_get_state` also consumes
      // a `pendingHotkey` press that arrived while this webview was booting.
      try {
        const result = await commands.assistantGetState();
        if (cancelled) return;
        if (result.status === "ok") {
          setState(result.data);
          // `handleHotkey` reads the snapshot via stateRef — seed it directly
          // because the render hasn't happened yet on this mount path.
          stateRef.current = result.data;
          if (result.data.pendingHotkey) handleHotkey();
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

      await add(() =>
        listen<DictatedPayload>("assistant://dictated", (event) => {
          setDraft((d) => appendDictated(d, event.payload.text));
          setLive("");
        }),
      );

      await add(() => listen("assistant://hotkey", () => handleHotkey()));

      // The streaming engine broadcasts committed+tentative text to every
      // window; the panel only shows it while this dictation is routed here.
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

  // The live preview is only "live" while a dictation is routed here; once
  // it stops, the finalized `assistant://dictated` normally replaces it —
  // give that delivery a short window before clearing a stale preview.
  useEffect(() => {
    if (state?.dictating !== false) return;
    if (!live) return;
    const id = window.setTimeout(() => setLive(""), DICTATED_SETTLE_MS);
    return () => window.clearTimeout(id);
  }, [state?.dictating, live]);

  // Keep the newest turn in view as messages/thinking land.
  useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [state?.messages, state?.phase]);

  // ---- Actions ------------------------------------------------------------

  const viewState: AssistantViewState = {
    phase: state?.phase ?? "idle",
    providerReady: state?.providerReady ?? false,
  };

  const onInputKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      send(draft);
    }
  };

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
    // The pin/close buttons inside the strip are not drag handles.
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

  const close = () => void commands.assistantClose();
  const cancel = () => void commands.assistantCancel();
  const retry = () => void commands.assistantRetry();
  const newConversation = () => {
    void commands.assistantNewConversation();
    setDraft("");
    setLive("");
    focusInput();
  };
  const openSettings = () => {
    void commands.showMainWindowCommand();
    // Lands on the Settings section's advanced tab, where the assistant
    // provider picker lives (`settingsTab` is read by SettingsHub).
    void emit("hub://navigate", {
      section: "settings",
      settingsTab: "advanced",
    });
  };

  // ---- Render -------------------------------------------------------------

  const dictating = state?.dictating ?? false;
  const pinned = state?.pinned ?? false;
  const phase = state?.phase ?? "idle";
  const messages = state?.messages ?? [];
  const sendable = canSend(draft, viewState);
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
            <span className="as-provider" title={state.providerId ?? undefined}>
              {t("assistant.provider", { label: state.providerLabel })}
            </span>
          )}
          <span className="as-title-spacer" />
          <button
            type="button"
            className={`as-pin${pinned ? " as-pin--on" : ""}`}
            aria-label={t(pinToggleKey(pinned))}
            aria-pressed={pinned}
            title={t(pinToggleKey(pinned))}
            onClick={togglePin}
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
            onClick={close}
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
          {phase === "thinking" && (
            <div className="as-msg as-assistant as-thinking" role="status">
              <span className="as-spinner" />
              {t("assistant.thinking")}
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

        <div className="as-composer">
          {dictating && (
            <div className="as-live" role="status">
              <span className="as-dot" />
              {t("assistant.listening")}
            </div>
          )}
          <textarea
            ref={inputRef}
            className="as-input"
            value={composerValue(draft, live, dictating)}
            readOnly={dictating}
            placeholder={t("assistant.inputPlaceholder")}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={onInputKeyDown}
            rows={2}
            aria-label={t("assistant.inputPlaceholder")}
          />
          <div className="as-composer-row">
            <button type="button" className="as-new" onClick={newConversation}>
              {t("assistant.newConversation")}
            </button>
            <span className="as-hint">{t("assistant.sendHint")}</span>
            {phase === "thinking" ? (
              <button type="button" className="as-send" onClick={cancel}>
                {t("assistant.cancel")}
              </button>
            ) : (
              <button
                type="button"
                className="as-send"
                disabled={!sendable}
                onClick={() => send(draft)}
              >
                {t("assistant.send")}
              </button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
};

export default AssistantPanel;
