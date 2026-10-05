import React, { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { commands, type LlmConnectionReport } from "@/bindings";
import { Button } from "../../ui/Button";

interface Props {
  providerId: string;
  model: string;
  configurationKey: string;
  disabled: boolean;
}

/** Checks saved configuration with a synthetic prompt, never meeting content. */
export const LlmConnectionTest: React.FC<Props> = ({
  providerId,
  model,
  configurationKey,
  disabled,
}) => {
  const { t } = useTranslation();
  const [pending, setPending] = useState(false);
  const [report, setReport] = useState<LlmConnectionReport | null>(null);
  const sequence = useRef(0);
  useEffect(() => {
    sequence.current += 1;
    setReport(null);
    setPending(false);
    return () => {
      sequence.current += 1;
    };
  }, [configurationKey]);

  const testModel = async () => {
    if (disabled || pending) return;
    const request = ++sequence.current;
    setPending(true);
    setReport(null);
    try {
      const result = await commands.testLlmConnection(providerId);
      if (request !== sequence.current) return;
      setReport(
        result.status === "ok"
          ? result.data
          : {
              ok: false,
              provider_id: providerId,
              latency_ms: null,
              kind: "provider",
            },
      );
    } catch {
      if (request === sequence.current)
        setReport({
          ok: false,
          provider_id: providerId,
          latency_ms: null,
          kind: "provider",
        });
    } finally {
      if (request === sequence.current) setPending(false);
    }
  };

  return (
    <div className="space-y-2 p-4">
      <Button
        type="button"
        variant="secondary"
        disabled={disabled || pending}
        onClick={() => void testModel()}
        aria-busy={pending}
      >
        {t(
          pending
            ? "settings.postProcessing.api.test.testing"
            : "settings.postProcessing.api.test.button",
        )}
      </Button>
      <p className="text-xs text-mid-gray">
        {t("settings.postProcessing.api.test.description")}
      </p>
      <p role="status" aria-live="polite" className="text-sm">
        {report &&
          (report.ok
            ? t("settings.postProcessing.api.test.success", {
                model:
                  model || t("settings.postProcessing.api.test.defaultModel"),
              })
            : t(
                `settings.postProcessing.api.test.errors.${report.kind ?? "provider"}`,
              ))}
      </p>
    </div>
  );
};
