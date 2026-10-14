import React, { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { Mic } from "lucide-react";
import { commands } from "@/bindings";
import { Button } from "../../ui/Button";
import { SetupRow, StatusLine } from "./SetupRow";

const BAR_COUNT = 12;
/** A level above this (0..1 meter) counts as "we heard you". */
const HEARD_LEVEL = 0.35;

type MicState = "idle" | "starting" | "testing" | "error";

interface MicTestRowProps {
  /** Persisted microphone name, `null` = system default. */
  selectedMicrophone: string | null;
}

/**
 * Microphone step: "Testar microfone" opens the mic and the bars follow the
 * voice (`onboarding://mic-level`). Hearing speech once marks the step done.
 */
export const MicTestRow: React.FC<MicTestRowProps> = ({
  selectedMicrophone,
}) => {
  const { t } = useTranslation();
  const [state, setState] = useState<MicState>("idle");
  const [levels, setLevels] = useState<number[]>(() =>
    Array(BAR_COUNT).fill(0),
  );
  const [heard, setHeard] = useState(false);
  const [deviceName, setDeviceName] = useState<string | null>(
    selectedMicrophone,
  );
  const unlisten = useRef<UnlistenFn[]>([]);

  const detach = () => {
    unlisten.current.forEach((fn) => fn());
    unlisten.current = [];
  };

  const stop = useCallback(async () => {
    detach();
    setLevels(Array(BAR_COUNT).fill(0));
    setState("idle");
    await commands.stopMicrophoneTest().catch(console.warn);
  }, []);

  // Never leave the mic open behind the screen.
  useEffect(
    () => () => {
      detach();
      void commands.stopMicrophoneTest().catch(console.warn);
    },
    [],
  );

  const start = async () => {
    setState("starting");
    detach();
    unlisten.current = await Promise.all([
      listen<{ level: number }>("onboarding://mic-level", (event) => {
        const level = event.payload.level;
        setLevels((prev) => [...prev.slice(1), level]);
        if (level >= HEARD_LEVEL) setHeard(true);
      }),
      listen("onboarding://mic-test-stopped", () => {
        detach();
        setLevels(Array(BAR_COUNT).fill(0));
        setState("idle");
      }),
    ]);
    const result = await commands.startMicrophoneTest();
    if (result.status === "ok") {
      setDeviceName(result.data.device_name);
      setState("testing");
    } else {
      detach();
      setState("error");
    }
  };

  const testing = state === "testing";
  const done = heard && !testing;

  return (
    <SetupRow
      tone={done ? "done" : "pending"}
      icon={<Mic className="w-5 h-5" />}
      title={t("onboarding.setup.mic.title")}
      description={deviceName ?? t("onboarding.setup.mic.defaultDevice")}
    >
      {done ? (
        <StatusLine tone="ok">{t("onboarding.setup.mic.verified")}</StatusLine>
      ) : (
        <>
          <div className="mt-2 flex items-center justify-between gap-3">
            <div
              className="flex items-end gap-[3px] h-[22px]"
              role="meter"
              aria-label={t("onboarding.setup.mic.meterLabel")}
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={Math.round((levels[BAR_COUNT - 1] ?? 0) * 100)}
            >
              {levels.map((level, index) => (
                <span
                  key={index}
                  className={`w-1 rounded-sm transition-[height] duration-75 ${
                    testing && level > 0.05 ? "bg-success" : "bg-mid-gray/25"
                  }`}
                  style={{ height: `${4 + Math.round(level * 18)}px` }}
                />
              ))}
            </div>
            {testing ? (
              <Button variant="secondary" onClick={() => void stop()}>
                {t("onboarding.setup.mic.stop")}
              </Button>
            ) : (
              <Button
                variant="primary"
                className="bg-logo-primary border-logo-primary hover:bg-logo-primary/90"
                disabled={state === "starting"}
                onClick={() => void start()}
              >
                {t("onboarding.setup.mic.test")}
              </Button>
            )}
          </div>
          {testing ? (
            <StatusLine tone={heard ? "ok" : "wait"}>
              {heard
                ? t("onboarding.setup.mic.hearing")
                : t("onboarding.setup.mic.speak")}
            </StatusLine>
          ) : state === "error" ? (
            <StatusLine tone="error">
              {t("onboarding.setup.mic.error")}
            </StatusLine>
          ) : null}
        </>
      )}
    </SetupRow>
  );
};
