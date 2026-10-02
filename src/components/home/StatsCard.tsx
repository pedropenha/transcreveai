import React from "react";
import { ArrowRight, Cpu, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { savedTimeParts } from "./homeView";

interface StatsCardProps {
  words: number;
  wordsPerMinute: number;
  streakDays: number;
  secondsSaved: number;
  activeModelName: string | null;
  tipVisible: boolean;
  onManageModels: () => void;
  onDismissTip: () => void;
}

/** Stats sidebar of Início: numbers in serif, saved time, active model, tip. */
export const StatsCard: React.FC<StatsCardProps> = ({
  words,
  wordsPerMinute,
  streakDays,
  secondsSaved,
  activeModelName,
  tipVisible,
  onManageModels,
  onDismissTip,
}) => {
  const { t, i18n } = useTranslation();
  const number = new Intl.NumberFormat(i18n.language);
  const { hours, minutes } = savedTimeParts(secondsSaved);
  const saved =
    hours > 0
      ? t("home.stats.savedHours", { hours, minutes })
      : t("home.stats.savedMinutes", { minutes });

  return (
    <aside
      className="home-stats"
      aria-label={t("settings.history.statistics.label")}
    >
      <div className="stat-section">
        <p className="stat-line">
          <b>{number.format(words)}</b>
          <span>{t("home.stats.words", { count: words })}</span>
        </p>
        <p className="stat-line">
          <b>{number.format(Math.round(wordsPerMinute))}</b>
          <span>{t("home.stats.wpm")}</span>
        </p>
        <p className="stat-line">
          <b>{number.format(streakDays)}</b>
          <span>{t("home.stats.streak", { count: streakDays })}</span>
        </p>
      </div>
      <div className="stat-section">
        <p className="stat-saved">{saved}</p>
        <p className="stat-small">{t("home.stats.savedCaption")}</p>
        <div className="model-mini">
          <span className="model-mini-icon" aria-hidden="true">
            <Cpu width={16} height={16} />
          </span>
          <div className="model-mini-text">
            <strong>{activeModelName ?? t("home.stats.noModel")}</strong>
            {activeModelName ? <span>{t("home.stats.local")}</span> : null}
          </div>
          <button
            type="button"
            className="icon-button"
            aria-label={t("home.stats.manageModels")}
            title={t("home.stats.manageModels")}
            onClick={onManageModels}
          >
            <ArrowRight width={14} height={14} aria-hidden="true" />
          </button>
        </div>
      </div>
      {tipVisible ? (
        <div className="stat-section home-tip" data-testid="home-tip">
          <button
            type="button"
            className="icon-button tip-close"
            aria-label={t("home.tip.dismiss")}
            onClick={onDismissTip}
          >
            <X width={14} height={14} aria-hidden="true" />
          </button>
          <p className="caps tip-label">{t("home.tip.label")}</p>
          <p className="tip-title">{t("home.tip.title")}</p>
          <p className="stat-small">{t("home.tip.body")}</p>
        </div>
      ) : null}
    </aside>
  );
};
