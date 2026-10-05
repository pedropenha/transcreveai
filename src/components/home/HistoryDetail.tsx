import React, { useMemo, useState } from "react";
import {
  Check,
  ClipboardPaste,
  Copy,
  Flag,
  RotateCcw,
  Trash2,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import type { HistoryEntry, ModelInfo } from "@/bindings";
import { buildWordDiff } from "../settings/history/historyView";
import { formatDuration } from "./format";
import { HistoryAppLogo } from "./HistoryAppLogo";
import { originLabel } from "./homeView";

function parseLatency(value: string): Record<string, string | number> {
  try {
    const parsed: unknown = JSON.parse(value);
    return parsed !== null && typeof parsed === "object"
      ? (parsed as Record<string, string | number>)
      : {};
  } catch {
    return {};
  }
}

export interface HistoryDetailProps {
  entry: HistoryEntry;
  retrying: boolean;
  retryModels: ModelInfo[];
  retryModelId: string;
  onRetryModelChange: (modelId: string) => void;
  onCopy: () => void;
  onReinsert: () => void;
  onRetry: () => void;
  onDelete: () => void;
  onToggleFlag: () => void;
  onAddDictionary: (term: string) => void;
}

export const HistoryDetail: React.FC<HistoryDetailProps> = ({
  entry,
  retrying,
  retryModels,
  retryModelId,
  onRetryModelChange,
  onCopy,
  onReinsert,
  onRetry,
  onDelete,
  onToggleFlag,
  onAddDictionary,
}) => {
  const { t, i18n } = useTranslation();
  const [dictionaryTerm, setDictionaryTerm] = useState("");
  const latency = useMemo(
    () => parseLatency(entry.latency_json),
    [entry.latency_json],
  );
  const diff = useMemo(
    () => buildWordDiff(entry.raw_text, entry.final_text),
    [entry.final_text, entry.raw_text],
  );
  const latencyLabel = (value: string | number | undefined) =>
    value === undefined
      ? t("settings.history.detail.unavailable")
      : t("settings.history.detail.milliseconds", { value });
  return (
    <>
      <p className="detail-label">{t("settings.history.detail.selected")}</p>
      <div className="detail-app">
        <HistoryAppLogo entry={entry} size="md" />
        <h2>{originLabel(entry) ?? t("settings.history.unknownApp")}</h2>
      </div>
      <p className="detail-meta">
        {new Date(entry.timestamp * 1000).toLocaleString(i18n.language)} ·{" "}
        {t(`settings.history.status.${entry.status}`)}
      </p>
      <label className="detail-retry-model">
        <span>{t("settings.history.retryModel")}</span>
        <select
          value={retryModelId}
          onChange={(event) => onRetryModelChange(event.target.value)}
          disabled={retrying || retryModels.length === 0}
        >
          {retryModels.map((model) => (
            <option key={model.id} value={model.id}>
              {model.name}
            </option>
          ))}
        </select>
      </label>
      <div className="detail-actions">
        <button
          type="button"
          onClick={onCopy}
          aria-label={t("settings.history.copyToClipboard")}
          title={t("settings.history.copyToClipboard")}
        >
          <Copy aria-hidden="true" />
        </button>
        <button
          type="button"
          onClick={onReinsert}
          aria-label={t("settings.history.reinsert")}
          title={t("settings.history.reinsert")}
        >
          <ClipboardPaste aria-hidden="true" />
        </button>
        <button
          type="button"
          onClick={onToggleFlag}
          aria-label={t("settings.history.flag")}
          aria-pressed={entry.saved}
          title={t("settings.history.flag")}
        >
          <Flag
            aria-hidden="true"
            fill={entry.saved ? "currentColor" : "none"}
          />
        </button>
        <button
          type="button"
          onClick={onRetry}
          disabled={!entry.audio_available || retrying}
          aria-label={t("settings.history.retranscribe")}
          title={t("settings.history.retranscribe")}
        >
          <RotateCcw
            aria-hidden="true"
            className={retrying ? "animate-spin" : ""}
          />
        </button>
        <button
          type="button"
          onClick={onDelete}
          aria-label={t("settings.history.delete")}
          title={t("settings.history.delete")}
        >
          <Trash2 aria-hidden="true" />
        </button>
      </div>
      <p className="detail-label">{t("settings.history.detail.final")}</p>
      <p className="detail-copy select-text">{entry.final_text}</p>
      <form
        className="detail-dictionary"
        onSubmit={(event) => {
          event.preventDefault();
          onAddDictionary(dictionaryTerm);
          setDictionaryTerm("");
        }}
      >
        <label htmlFor={`history-dictionary-${entry.id}`} className="sr-only">
          {t("settings.history.dictionaryTermLabel")}
        </label>
        <input
          id={`history-dictionary-${entry.id}`}
          value={dictionaryTerm}
          onChange={(event) => setDictionaryTerm(event.target.value)}
          placeholder={t("settings.history.dictionaryTermPlaceholder")}
        />
        <button type="submit">{t("settings.history.addDictionary")}</button>
      </form>
      <p className="detail-label">{t("settings.history.detail.diff")}</p>
      <p className="detail-diff select-text">
        {diff.map((part, index) => (
          <span key={`${part.kind}-${index}`} className={`diff-${part.kind}`}>
            {part.kind !== "equal" ? (
              <span className="sr-only">
                {t(`settings.history.detail.${part.kind}`)}:{" "}
              </span>
            ) : null}
            {part.kind === "added" ? <Check aria-hidden="true" /> : null}
            {part.text}{" "}
          </span>
        ))}
      </p>
      <p className="detail-label">{t("settings.history.detail.pipeline")}</p>
      <dl className="detail-grid">
        <div>
          <dt>{t("settings.history.detail.provider")}</dt>
          <dd>
            {retryModels.find((model) =>
              entry.stt_provider_id?.endsWith(model.id),
            )?.name ||
              entry.stt_provider_id ||
              t("settings.history.localProvider")}
          </dd>
        </div>
        <div>
          <dt>{t("settings.history.detail.language")}</dt>
          <dd>{entry.language || t("settings.history.detail.unavailable")}</dd>
        </div>
        <div>
          <dt>STT</dt>
          <dd>{latencyLabel(latency.stt_ms)}</dd>
        </div>
        <div>
          <dt>LLM</dt>
          <dd>{latencyLabel(latency.llm_ms)}</dd>
        </div>
        <div>
          <dt>{t("settings.history.detail.insertion")}</dt>
          <dd>{latencyLabel(latency.insert_ms)}</dd>
        </div>
        <div>
          <dt>{t("settings.history.detail.duration")}</dt>
          <dd>{formatDuration(entry.duration_ms / 1000, i18n.language)}</dd>
        </div>
      </dl>
    </>
  );
};
