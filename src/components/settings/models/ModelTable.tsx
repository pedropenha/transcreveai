import React from "react";
import { Check, Download, Loader2, Trash2, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { ModelInfo, Suitability } from "@/bindings";
import {
  getLanguageDisplayText,
  isLegacySource,
  type ModelCardStatus,
} from "@/components/onboarding";
import type { SttUsage } from "@/lib/providers";
import { formatModelSize } from "@/lib/utils/format";
import {
  getTranslatedModelDescription,
  getTranslatedModelName,
} from "@/lib/utils/modelTranslation";
import { canTranslateToEnglish } from "../translationModels";
import { Dots } from "./Dots";
import { SCORE_DOTS } from "./modelsView";

export interface ModelTableHandlers {
  onSelect: (id: string) => void;
  onDownload: (id: string) => void;
  onDelete: (id: string) => void;
  onCancel: (id: string) => void;
}

export interface ModelRowState {
  model: ModelInfo;
  status: ModelCardStatus;
  /** Percentage 0-100 while downloading. */
  progress?: number;
  /** MB/s while downloading. */
  speed?: number;
  suitability?: Suitability;
  usages?: SttUsage[];
}

interface ModelTableProps extends ModelTableHandlers {
  rows: readonly ModelRowState[];
  caption: string;
}

/** Catalog table: model, languages, size, speed, accuracy and the action. */
export const ModelTable: React.FC<ModelTableProps> = ({
  rows,
  caption,
  ...handlers
}) => {
  const { t } = useTranslation();
  return (
    <table className="st-mtable">
      <caption className="sr-only">{caption}</caption>
      <thead>
        <tr>
          <th scope="col">{t("settings.models.columns.model")}</th>
          <th scope="col" className="st-col-lang">
            {t("settings.models.columns.languages")}
          </th>
          <th scope="col">{t("settings.models.columns.size")}</th>
          <th scope="col">{t("settings.models.columns.speed")}</th>
          <th scope="col" className="st-col-acc">
            {t("settings.models.columns.accuracy")}
          </th>
          <th scope="col">
            <span className="sr-only">
              {t("settings.models.columns.actions")}
            </span>
          </th>
        </tr>
      </thead>
      <tbody>
        {rows.map((row) => (
          <ModelRow key={row.model.id} row={row} {...handlers} />
        ))}
      </tbody>
    </table>
  );
};

const ModelRow: React.FC<{ row: ModelRowState } & ModelTableHandlers> = ({
  row,
  ...handlers
}) => {
  const { t } = useTranslation();
  const { model, status } = row;
  const name = getTranslatedModelName(model, t);
  const scoreLabel = (metricKey: string, score: number) =>
    t("settings.models.scoreLabel", {
      metric: t(metricKey),
      value: Math.round(score * SCORE_DOTS),
    });

  return (
    <tr data-status={status} data-model-id={model.id}>
      <th scope="row" className="st-col-name">
        <span className="st-model-name">
          {name}
          {model.is_recommended && !model.is_downloaded ? (
            <Tag tone="outline">{t("onboarding.recommended")}</Tag>
          ) : null}
          {model.is_custom ? (
            <Tag tone="outline">{t("modelSelector.custom")}</Tag>
          ) : null}
          {isLegacySource(model) ? (
            <Tag tone="outline">{t("modelSelector.legacy")}</Tag>
          ) : null}
          {canTranslateToEnglish(model) ? (
            <Tag tone="outline">{t("settings.models.translatesTag")}</Tag>
          ) : null}
          {row.suitability === "good_fit" ? (
            <Tag tone="success">{t("settings.models.suitability.goodFit")}</Tag>
          ) : null}
          {row.suitability === "heavy" ? (
            <Tag tone="warning">{t("settings.models.suitability.heavy")}</Tag>
          ) : null}
          {row.suitability === "not_advised" ? (
            <Tag tone="warning">
              {t("settings.models.suitability.notAdvised")}
            </Tag>
          ) : null}
          {row.usages?.map((usage) => (
            <Tag key={usage} tone="accent">
              {t(`settings.models.usage.${usage}`)}
            </Tag>
          ))}
        </span>
        <span className="st-model-desc">
          {getTranslatedModelDescription(model, t)}
        </span>
      </th>
      <td className="st-col-lang">
        {getLanguageDisplayText(model.supported_languages, t)}
      </td>
      <td className="st-num">{formatModelSize(Number(model.size_mb))}</td>
      <td>
        <Dots
          score={model.speed_score}
          label={scoreLabel("settings.models.columns.speed", model.speed_score)}
        />
      </td>
      <td className="st-col-acc">
        <Dots
          score={model.accuracy_score}
          label={scoreLabel(
            "settings.models.columns.accuracy",
            model.accuracy_score,
          )}
        />
      </td>
      <td className="st-col-act">
        <RowAction row={row} name={name} {...handlers} />
      </td>
    </tr>
  );
};

const Tag: React.FC<{
  tone: "outline" | "success" | "warning" | "accent";
  children: React.ReactNode;
}> = ({ tone, children }) => (
  <span className="st-chip st-chip-sm" data-tone={tone}>
    {children}
  </span>
);

const BusyChip: React.FC<{ labelKey: string }> = ({ labelKey }) => {
  const { t } = useTranslation();
  return (
    <div className="st-act">
      <span className="st-chip" data-tone="accent">
        <Loader2
          width={12}
          height={12}
          className="animate-spin"
          aria-hidden="true"
        />
        {t(labelKey)}
      </span>
    </div>
  );
};

const RowAction: React.FC<
  { row: ModelRowState; name: string } & ModelTableHandlers
> = ({ row, name, onSelect, onDownload, onDelete, onCancel }) => {
  const { t } = useTranslation();
  const { model, status } = row;

  if (status === "downloading") {
    const pct = Math.round(row.progress ?? 0);
    const speed =
      row.speed && row.speed > 0
        ? ` · ${t("modelSelector.downloadSpeed", { speed: row.speed.toFixed(1) })}`
        : "";
    return (
      <div className="st-act">
        <div className="st-dl">
          <div className="st-dl-row">
            <span>{t("settings.models.row.downloading")}</span>
            <span>{`${pct}%${speed}`}</span>
          </div>
          <div
            className="st-progress"
            role="progressbar"
            aria-label={t("modelSelector.downloading", { percentage: pct })}
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={pct}
          >
            <i style={{ width: `${pct}%` }} />
          </div>
        </div>
        <button
          type="button"
          className="st-icon-btn"
          aria-label={t("modelSelector.cancelDownload")}
          title={t("modelSelector.cancelDownload")}
          onClick={() => onCancel(model.id)}
        >
          <X width={14} height={14} aria-hidden="true" />
        </button>
      </div>
    );
  }
  if (status === "verifying") {
    return <BusyChip labelKey="modelSelector.verifyingGeneric" />;
  }
  if (status === "extracting") {
    return <BusyChip labelKey="modelSelector.extractingGeneric" />;
  }
  if (status === "switching") {
    return <BusyChip labelKey="modelSelector.switching" />;
  }
  if (status === "downloadable") {
    return (
      <div className="st-act">
        <button
          type="button"
          className="st-btn st-btn-secondary"
          aria-label={t("settings.models.row.downloadNamed", {
            modelName: name,
          })}
          onClick={() => onDownload(model.id)}
        >
          <Download width={13} height={13} aria-hidden="true" />
          {t("settings.models.row.download")}
        </button>
      </div>
    );
  }
  return (
    <div className="st-act">
      <span className="st-chip" data-tone="success">
        <Check width={12} height={12} aria-hidden="true" />
        {t("settings.models.row.downloaded")}
      </span>
      {status === "available" ? (
        <button
          type="button"
          className="st-btn st-btn-soft"
          aria-label={t("settings.models.row.useNamed", { modelName: name })}
          onClick={() => onSelect(model.id)}
        >
          {t("settings.models.row.use")}
        </button>
      ) : null}
      <button
        type="button"
        className="st-icon-btn"
        aria-label={t("modelSelector.deleteModel", { modelName: name })}
        title={t("modelSelector.deleteModel", { modelName: name })}
        onClick={() => onDelete(model.id)}
      >
        <Trash2 width={14} height={14} aria-hidden="true" />
      </button>
    </div>
  );
};
