import React from "react";
import { Cpu, Monitor } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { ModelInfo } from "@/bindings";
import { getLanguageDisplayText } from "@/components/onboarding";
import { formatModelSize } from "@/lib/utils/format";
import {
  getTranslatedModelDescription,
  getTranslatedModelName,
} from "@/lib/utils/modelTranslation";
import { Dots } from "./Dots";
import { SCORE_DOTS, type AccelerationSummary } from "./modelsView";

interface ModelHeroProps {
  /** The active, downloaded model, or null when none is usable. */
  model: ModelInfo | null;
  acceleration: AccelerationSummary;
}

/** "Em uso": the active model with its metrics and the acceleration in use. */
export const ModelHero: React.FC<ModelHeroProps> = ({
  model,
  acceleration,
}) => {
  const { t } = useTranslation();
  if (model === null) {
    return (
      <div className="st-hero st-hero-empty" data-testid="model-hero">
        <span className="st-hero-icon" aria-hidden="true">
          <Cpu width={22} height={22} />
        </span>
        <div>
          <h3>{t("settings.models.hero.noneTitle")}</h3>
          <p className="st-hero-desc">{t("settings.models.hero.noneBody")}</p>
        </div>
      </div>
    );
  }
  const scoreLabel = (metricKey: string, score: number) =>
    t("settings.models.scoreLabel", {
      metric: t(metricKey),
      value: Math.round(score * SCORE_DOTS),
    });
  return (
    <div className="st-hero" data-testid="model-hero">
      <span className="st-hero-icon" aria-hidden="true">
        <Cpu width={22} height={22} />
      </span>
      <div className="st-hero-body">
        <h3>
          {getTranslatedModelName(model, t)}
          <span className="st-chip" data-tone="accent">
            <span className="st-chip-dot" aria-hidden="true" />
            {t("modelSelector.active")}
          </span>
          {model.is_recommended ? (
            <span className="st-chip" data-tone="urucum">
              {t("onboarding.recommended")}
            </span>
          ) : null}
        </h3>
        <p className="st-hero-desc">
          {getTranslatedModelDescription(model, t)}
        </p>
        <dl className="st-metrics">
          <div>
            <dt>{t("settings.models.columns.size")}</dt>
            <dd>{formatModelSize(Number(model.size_mb))}</dd>
          </div>
          <div>
            <dt>{t("settings.models.columns.speed")}</dt>
            <dd>
              <Dots
                score={model.speed_score}
                label={scoreLabel(
                  "settings.models.columns.speed",
                  model.speed_score,
                )}
              />
            </dd>
          </div>
          <div>
            <dt>{t("settings.models.columns.accuracy")}</dt>
            <dd>
              <Dots
                score={model.accuracy_score}
                label={scoreLabel(
                  "settings.models.columns.accuracy",
                  model.accuracy_score,
                )}
              />
            </dd>
          </div>
          <div>
            <dt>{t("settings.models.columns.languages")}</dt>
            <dd>{getLanguageDisplayText(model.supported_languages, t)}</dd>
          </div>
        </dl>
      </div>
      <div className="st-hw">
        <Monitor width={15} height={15} aria-hidden="true" />
        <span>{t("settings.models.hero.acceleration")}</span>
        <b>
          {acceleration.kind === "gpu"
            ? t("settings.models.hero.gpu", { device: acceleration.device })
            : t("settings.models.hero.cpu")}
        </b>
      </div>
    </div>
  );
};
