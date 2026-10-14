import React from "react";
import { useTranslation } from "react-i18next";
import { AudioLines } from "lucide-react";
import { formatModelSize } from "../../../lib/utils/format";
import Badge from "../../ui/Badge";
import { Button } from "../../ui/Button";
import { SetupRow, StatusLine } from "./SetupRow";
import type { SetupModelState } from "./useSetupModel";

/** Transcription model step: Parakeet V3 downloads by itself, no choice. */
export const ModelSetupRow: React.FC<{ state: SetupModelState }> = ({
  state,
}) => {
  const { t } = useTranslation();
  const { phase, model, progress, speed, downloadedMb, retry } = state;
  const sizeMb = model ? Number(model.size_mb) : undefined;
  const tone = phase === "ready" ? "done" : phase === "error" ? "warn" : "busy";

  return (
    <SetupRow
      tone={tone}
      icon={<AudioLines className="w-5 h-5" />}
      title={t("onboarding.setup.model.title")}
      badge={
        <Badge variant="primary">{t("onboarding.setup.model.badge")}</Badge>
      }
      description={
        phase === "ready" ? undefined : t("onboarding.setup.model.description")
      }
    >
      {phase === "ready" ? (
        <StatusLine tone="ok">
          {sizeMb
            ? t("onboarding.setup.model.installedSize", {
                size: formatModelSize(sizeMb),
              })
            : t("onboarding.setup.model.installed")}
        </StatusLine>
      ) : phase === "error" ? (
        <>
          <StatusLine tone="error">
            {t("onboarding.setup.model.error")}
          </StatusLine>
          <Button variant="secondary" className="mt-2" onClick={retry}>
            {t("onboarding.setup.model.retry")}
          </Button>
        </>
      ) : (
        <div className="mt-2">
          <div className="w-full h-1.5 bg-mid-gray/20 rounded-full overflow-hidden">
            <div
              className={`h-full bg-logo-primary rounded-full transition-all duration-300 ${
                phase === "downloading" ? "" : "animate-pulse w-full"
              }`}
              style={
                phase === "downloading"
                  ? { width: `${Math.round(progress ?? 0)}%` }
                  : undefined
              }
            />
          </div>
          <div className="flex justify-between text-xs text-text/50 mt-1 tabular-nums">
            <span>
              {phase === "downloading"
                ? downloadedMb !== undefined && sizeMb
                  ? t("onboarding.setup.model.downloadingOf", {
                      percentage: Math.round(progress ?? 0),
                      done: Math.round(downloadedMb),
                      total: Math.round(sizeMb),
                    })
                  : t("onboarding.setup.model.downloading", {
                      percentage: Math.round(progress ?? 0),
                    })
                : t(`onboarding.setup.model.${phase}`)}
            </span>
            {phase === "downloading" && speed ? (
              <span>
                {t("modelSelector.downloadSpeed", { speed: speed.toFixed(1) })}
              </span>
            ) : null}
          </div>
        </div>
      )}
    </SetupRow>
  );
};
