import React, { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Keyboard } from "lucide-react";
import { commands } from "@/bindings";
import { useOsType } from "@/hooks/useOsType";
import {
  formatKeyCombination,
  getKeyName,
  normalizeKey,
} from "../../../lib/utils/keyboard";
import { GlobalShortcutInput } from "../../settings/GlobalShortcutInput";
import { Button } from "../../ui/Button";
import { hitKeys, shortcutKeys, shortcutMatched } from "../setupFlow";
import { SetupRow, StatusLine } from "./SetupRow";

type Mode = "testing" | "editing" | "done";

interface ShortcutTestRowProps {
  /** `bindings.transcribe.current_binding`, e.g. "ctrl+space". */
  binding: string | undefined;
}

/**
 * Shortcut step: asks the user to press the dictation shortcut and lights up
 * each key as it is recognized. On first run global shortcuts are not
 * registered yet (App initializes them after onboarding), so the press simply
 * reaches this window. "Trocar atalho" needs them registered to save a new
 * binding; from then on the test suspends them so the press does not start a
 * dictation.
 */
export const ShortcutTestRow: React.FC<ShortcutTestRowProps> = ({
  binding,
}) => {
  const { t } = useTranslation();
  const osType = useOsType();
  const keys = useMemo(() => shortcutKeys(binding), [binding]);
  const [mode, setMode] = useState<Mode>("testing");
  const [pressed, setPressed] = useState<Set<string>>(() => new Set());
  const registered = useRef(false);

  useEffect(() => {
    if (mode !== "testing" || keys.length === 0) return;
    const suspend = registered.current;
    if (suspend) void commands.suspendAllBindings().catch(console.warn);
    const keyOf = (e: KeyboardEvent) => normalizeKey(getKeyName(e, osType));
    const down = (e: KeyboardEvent) => {
      e.preventDefault();
      setPressed((prev) => new Set(prev).add(keyOf(e)));
    };
    const up = (e: KeyboardEvent) => {
      e.preventDefault();
      const key = keyOf(e);
      setPressed((prev) => {
        const next = new Set(prev);
        next.delete(key);
        return next;
      });
    };
    // Releasing keys outside the window would leave them "held".
    const reset = () => setPressed(new Set());
    window.addEventListener("keydown", down);
    window.addEventListener("keyup", up);
    window.addEventListener("blur", reset);
    return () => {
      window.removeEventListener("keydown", down);
      window.removeEventListener("keyup", up);
      window.removeEventListener("blur", reset);
      if (suspend) void commands.resumeAllBindings().catch(console.warn);
    };
  }, [mode, keys, osType]);

  useEffect(() => {
    if (mode === "testing" && shortcutMatched(keys, pressed)) {
      setMode("done");
    }
  }, [mode, keys, pressed]);

  const startEditing = async () => {
    try {
      await commands.initializeShortcuts();
      registered.current = true;
    } catch (e) {
      console.warn("initialize_shortcuts failed:", e);
    }
    setMode("editing");
  };

  const hits = mode === "done" ? new Set(keys) : hitKeys(keys, pressed);

  return (
    <SetupRow
      tone={mode === "done" ? "done" : "pending"}
      icon={<Keyboard className="w-5 h-5" />}
      title={t("onboarding.setup.shortcut.title")}
      description={
        mode === "done" ? undefined : t("onboarding.setup.shortcut.description")
      }
    >
      {mode === "editing" ? (
        <div className="mt-2 flex flex-col gap-2">
          <GlobalShortcutInput shortcutId="transcribe" grouped />
          <Button
            variant="secondary"
            className="self-start"
            onClick={() => {
              setPressed(new Set());
              setMode("testing");
            }}
          >
            {t("onboarding.setup.shortcut.testAgain")}
          </Button>
        </div>
      ) : (
        <>
          <div className="mt-2 flex items-center justify-between gap-3">
            <div className="flex items-center gap-1.5" aria-hidden="true">
              {keys.map((key, index) => (
                <React.Fragment key={key}>
                  {index > 0 ? <span className="text-text/50">+</span> : null}
                  <kbd
                    className={`font-mono text-[13px] px-2.5 py-1 rounded-md border border-b-2 ${
                      hits.has(key)
                        ? "bg-success-soft border-success text-success"
                        : "bg-logo-primary/10 border-logo-primary text-logo-primary animate-pulse"
                    }`}
                  >
                    {formatKeyCombination(key, osType)}
                  </kbd>
                </React.Fragment>
              ))}
            </div>
            {mode === "testing" ? (
              <Button
                variant="ghost"
                className="text-logo-primary"
                onClick={() => void startEditing()}
              >
                {t("onboarding.setup.shortcut.change")}
              </Button>
            ) : null}
          </div>
          <StatusLine tone={mode === "done" ? "ok" : "wait"}>
            {mode === "done"
              ? t("onboarding.setup.shortcut.verified")
              : t("onboarding.setup.shortcut.waiting", {
                  shortcut: formatKeyCombination(binding ?? "", osType),
                })}
          </StatusLine>
        </>
      )}
    </SetupRow>
  );
};
