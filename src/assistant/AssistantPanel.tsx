import React, { useCallback, useEffect, useRef, useState } from "react";
import { emit, listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import ReactMarkdown from "react-markdown";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useTranslation } from "react-i18next";
import { MessageSquarePlus, Pin, PinOff, Sparkles, X, Eye } from "lucide-react";
import { AssistantComposer } from "./AssistantComposer";
import { useAssistantInput } from "./useAssistantInput";
import "./AssistantPanel.css";
import { commands, events } from "@/bindings";
import type { AssistantStateEvent } from "@/bindings";
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

/** Run a specta command and make failures visible in the log — a
 *  silently-failed strip action looks exactly like a dead button. `false`
 *  lets the caller recover (e.g. `close` still hides the window). */
async function invokeChecked<T, E>(
  name: string,
  call: () => Promise<
    { status: "ok"; data: T } | { status: "error"; error: E }
  >,
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

const AssistantPanel: React.FC = () => {
  const { t } = useTranslation();
  const direction = getLanguageDirection(i18n.language);

  const [state, setState] = useState<AssistantStateEvent | null>(null);
  const input = useAssistantInput(state);
  const [actionError, setActionError] = useState<string | null>(null);
  const [pendingAction, setPendingAction] = useState<string | null>(null);
  const pendingActionRef = useRef<string | null>(null);
  const [solid, setSolid] = useState(() => {
    try {
      return localStorage.getItem("transcreve-ai.assistant-solid") === "true";
    } catch {
      return true;
    }
  });
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
  // The view-model decisions read the freshest state inside event
  // handlers. Ref writes live in an effect so render stays side-effect
  // free (StrictMode); the hydration path still writes it inline — that
  // assignment is the apply itself, not a render side effect.
  const stateRef = useRef<AssistantStateEvent | null>(null);
  const stateEventRevision = useRef(0);
  useEffect(() => {
    stateRef.current = state;
  });

  useEffect(() => {
    const unlisteners: (() => void)[] = [];
    let cancelled = false;

    const setup = async () => {
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

      // Listeners first, hydrate second: an `assistant://state` emitted
      // between the snapshot fetch and the listener registration would
      // otherwise be lost and the snapshot would already be stale. An
      // event that arrives *after* registration is fresher than the
      // in-flight snapshot — hydration must not overwrite it.
      let eventDelivered = false;
      try {
        await add(() =>
          listen<AssistantStateEvent>("assistant://state", (event) => {
            eventDelivered = true;
            stateEventRevision.current += 1;
            stateRef.current = event.payload;
            setState(event.payload);
          }),
        );

        // The streaming engine broadcasts committed+tentative text to every
        // window; the panel only shows it while this dictation is routed here.
        await add(() =>
          events.streamTextEvent.listen((event) => {
            if (stateRef.current?.dictating) {
              const p = event.payload;
              setLive(`${p.committed}${p.tentative ? ` ${p.tentative}` : ""}`);
            }
          }),
        );
      } catch (e) {
        console.warn("Assistant listeners failed to register:", e);
      }

      // Hydrate so a cold webview renders the full snapshot (open
      // conversation, pinned state, provider chip). Skip the apply when a
      // state event already landed — the snapshot was fetched before it.
      try {
        const result = await commands.assistantGetState();
        if (cancelled || eventDelivered) return;
        if (result.status === "ok") {
          setState(result.data);
          stateRef.current = result.data;
        }
      } catch (e) {
        // The panel can still render its chrome without a snapshot; the next
        // `assistant://state` event recovers it.
        console.debug("Assistant state hydration failed:", e);
      }
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
      void runAction("dismiss", () => commands.assistantDismiss());
    }
  };

  // FR-012-11: the panel never takes focus on its own — a click is explicit
  // intent, so the backend makes the window key/focused here.
  const onPanelMouseDown = () => {
    void invokeChecked("assistant_focus", () => commands.assistantFocus());
  };

  const runAction = async (
    id: string,
    call: () => ReturnType<typeof commands.assistantClose>,
  ) => {
    if (pendingActionRef.current) return false;
    pendingActionRef.current = id;
    setPendingAction(id);
    setActionError(null);
    try {
      const ok = await invokeChecked(`assistant_${id}`, call);
      if (!ok) setActionError(`assistant.actionError.${id}`);
      return ok;
    } finally {
      pendingActionRef.current = null;
      setPendingAction(null);
    }
  };

  const toggleSolid = () => {
    const next = !solid;
    setSolid(next);
    try {
      localStorage.setItem("transcreve-ai.assistant-solid", String(next));
    } catch {
      /* The in-memory preference still works without storage. */
    }
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
    if (!(e.target instanceof Element) || e.target.closest("button")) return;
    grabRef.current = { clientX: e.clientX, clientY: e.clientY };
    try {
      e.currentTarget.setPointerCapture(e.pointerId);
    } catch {
      // InvalidStateError when the pointer is already gone — the drag
      // just won't start rather than surfacing a crash.
      grabRef.current = null;
    }
  };

  const onTitlePointerMove = () => {
    const grab = grabRef.current;
    if (!isPanelDragging(grab) || !grab) return;
    if (dragFrameRef.current !== 0) return;
    const { clientX, clientY } = grab;
    dragFrameRef.current = requestAnimationFrame(() => {
      dragFrameRef.current = 0;
      void invokeChecked("assistant_move_panel", () =>
        commands.assistantMovePanel(clientX, clientY),
      );
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
    void invokeChecked("assistant_save_panel_position", () =>
      commands.assistantSavePanelPosition(grab.clientX, grab.clientY),
    );
  };

  // Pinned panels remain draggable; the backend docks them on release.
  const togglePin = () => {
    void runAction("pin", async () => {
      const result = await commands.assistantSetPanelPinned(
        !(stateRef.current?.pinned ?? false),
      );
      if (result.status === "error") return result;
      // Command acknowledgement is not a state snapshot; wait for confirmed
      // state before allowing the next toggle, even if its event was delayed.
      const revision = stateEventRevision.current;
      const snapshot = await commands.assistantGetState();
      if (snapshot.status === "ok" && stateEventRevision.current === revision) {
        setState(snapshot.data);
        stateRef.current = snapshot.data;
      } else if (snapshot.status === "error") return snapshot;
      return result;
    });
  };

  const close = useCallback(() => {
    void invokeChecked("assistant_close", () => commands.assistantClose())
      .then((ok) => {
        if (ok) return;
        // The X must never look dead: hide the window locally so the
        // gesture always lands; the backend session keeps its state.
        try {
          void getCurrentWindow()
            .hide()
            .catch((e) => {
              console.warn("assistant panel hide failed:", e);
              setActionError("assistant.actionError.close");
            });
        } catch (e) {
          console.warn("assistant panel hide failed:", e);
          setActionError("assistant.actionError.close");
        }
      })
      .catch((e) => console.warn("assistant close fallback failed:", e));
  }, []);
  const cancel = () =>
    void runAction("cancel", () => commands.assistantCancel());
  const retry = () => void runAction("retry", () => commands.assistantRetry());
  const newConversation = () => {
    if (input.sending) return;
    const revision = input.getRevision();
    void runAction("newConversation", () =>
      commands.assistantNewConversation(),
    ).then((ok) => {
      if (ok) {
        setLive("");
        input.reset(revision);
      }
    });
  };
  const dictate = () =>
    void runAction("dictate", () => commands.assistantToggleDictation());
  const openSettings = () => {
    void invokeChecked("show_main_window", () =>
      commands.showMainWindowCommand(),
    );
    // Lands on Settings → Intelligence → Assistant, where the assistant
    // provider picker lives (`settingsTab` is read by SettingsHub).
    void emit("hub://navigate", {
      section: "settings",
      settingsTab: "intelligence/assistant",
    }).catch((e) => console.warn("hub://navigate emit failed:", e));
  };

  /** Title-strip buttons act on pointerdown AND click, deduped *per
   *  gesture*: pointerdown runs the action and stamps it consumed; the
   *  paired click is suppressed. An unpaired click (keyboard, or a
   *  pointerdown the WebView2 focus transition swallowed) still runs.
   *  The stamp is re-taken on pointerup — the click lands at release, so
   *  a held press must measure freshness from the release, not the press —
   *  and carries a freshness bound because `preventDefault` on pointerdown
   *  may suppress the paired click entirely (without it a never-consumed
   *  flag could eat one later keyboard click). `pointercancel` clears it
   *  (a cancelled press makes no click); `pointerleave` never does — a
   *  mouse press that leaves, re-enters and releases still pairs with a
   *  click, and a touch/pen release fires leave *before* the click, so
   *  clearing on either would double-run. */
  const consumedStripGestureRef = useRef<Partial<Record<string, number>>>({});
  const stripButton = (id: string, action: () => void) => {
    const clear = () => {
      delete consumedStripGestureRef.current[id];
    };
    const restamp = () => {
      if (consumedStripGestureRef.current[id] !== undefined) {
        consumedStripGestureRef.current[id] = performance.now();
      }
    };
    return {
      onPointerDown: (e: React.PointerEvent) => {
        if (e.button !== 0) return; // only the primary button activates
        if (
          e.currentTarget instanceof HTMLButtonElement &&
          e.currentTarget.disabled
        )
          return;
        e.preventDefault();
        e.stopPropagation();
        consumedStripGestureRef.current[id] = performance.now();
        action();
      },
      onPointerUp: restamp,
      onPointerCancel: clear,
      onMouseDown: (e: React.MouseEvent) => e.stopPropagation(),
      onClick: (e: React.MouseEvent) => {
        e.stopPropagation();
        const stamp = consumedStripGestureRef.current[id];
        clear();
        // The click paired with a pointerdown that already ran is the
        // duplicate; a stale flag (> 1 s — the paired click never arrived)
        // must not suppress a fresh gesture.
        if (stamp !== undefined && performance.now() - stamp < 1000) {
          return;
        }
        action();
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
    <main
      className={`as-stage${solid ? " as-stage--solid" : ""}`}
      dir={direction}
      onMouseDown={onPanelMouseDown}
      onKeyDown={onStageKeyDown}
    >
      {/* Persistent live region — a region inserted together with its
          content is often not announced, and the streaming preview text
          would re-announce on every STT event. "Listening" lands once
          here instead. */}
      <span className="as-sr" aria-live="polite">
        {dictating ? t("assistant.listening") : ""}
      </span>
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
          <span className="as-mark">
            <Sparkles size={18} className="as-title-icon" aria-hidden="true" />
          </span>
          <span className="as-title-text">{t("assistant.title")}</span>
          <span className="as-title-spacer" />
          <button
            type="button"
            className="as-pin"
            aria-label={t("assistant.newConversation")}
            title={t("assistant.newConversation")}
            disabled={pendingAction !== null || input.sending || state === null}
            {...stripButton("new", newConversation)}
          >
            <MessageSquarePlus size={18} aria-hidden="true" />
          </button>
          <button
            type="button"
            className={`as-pin${pinned ? " as-pin--on" : ""}`}
            // One stable label — `aria-pressed` carries the state, so the
            // announcement is "Pin, pressed" rather than a contradictory
            // "Unpin, pressed". The tooltip still names the pending action.
            aria-label={t("assistant.pin")}
            aria-pressed={pinned}
            title={t(pinToggleKey(pinned))}
            disabled={pendingAction !== null || state === null}
            {...stripButton("pin", togglePin)}
          >
            {pinned ? (
              <PinOff size={18} aria-hidden="true" />
            ) : (
              <Pin size={18} aria-hidden="true" />
            )}
          </button>
          <button
            type="button"
            className="as-x"
            aria-label={t("assistant.close")}
            {...stripButton("close", close)}
          >
            <X size={18} aria-hidden="true" />
          </button>
        </header>

        <div className="as-meta">
          <button
            type="button"
            className={`as-provider${experimental ? " as-provider--experimental" : ""}`}
            title={state?.providerId ?? undefined}
            aria-label={t("assistant.changeProvider")}
            onClick={openSettings}
          >
            {state?.providerLabel
              ? t("assistant.provider", { label: state.providerLabel })
              : t("assistant.noProviderLabel")}
            {experimental && (
              <em className="as-exp"> {t("assistant.experimental")}</em>
            )}
          </button>
          <button
            type="button"
            className="as-material"
            aria-label={t("assistant.reduceTransparency")}
            title={t("assistant.reduceTransparency")}
            aria-pressed={solid}
            onClick={toggleSolid}
          >
            <Eye size={16} aria-hidden="true" />
          </button>
        </div>

        {(actionError || input.errorKey) && (
          <div className="as-action-error" role="alert">
            {t(actionError ?? input.errorKey ?? "assistant.actionError.send")}
          </div>
        )}

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
              <span className="as-empty-mark">
                <Sparkles size={27} aria-hidden="true" />
              </span>
              <h1>{t("assistant.emptyTitle")}</h1>
              <p>{t("assistant.empty")}</p>
              <p className="as-privacy">{t("assistant.privacyHint")}</p>
              <div className="as-suggestions">
                {["review", "organize"].map((kind) => (
                  <button
                    type="button"
                    key={kind}
                    onClick={() => {
                      input.change(t(`assistant.suggestion.${kind}`));
                      input.inputRef.current?.focus();
                    }}
                  >
                    {t(`assistant.suggestion.${kind}`)}
                  </button>
                ))}
              </div>
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
                            if (href)
                              void openUrl(href).catch((err) =>
                                console.warn(
                                  "assistant link open failed:",
                                  err,
                                ),
                              );
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
            <div className="as-msg as-user as-live-msg">
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

        <AssistantComposer
          draft={input.draft}
          inputRef={input.inputRef}
          onChange={input.change}
          onSend={() => void input.send()}
          onDictate={dictate}
          dictating={dictating}
          canSend={input.canSend && pendingAction === null}
          canDictate={
            pendingAction === null &&
            Boolean(state && (dictating || state.providerReady))
          }
        />
      </div>
    </main>
  );
};

export default AssistantPanel;
