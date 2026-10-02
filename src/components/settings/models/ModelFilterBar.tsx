import React from "react";
import {
  AudioLines,
  FolderInput,
  Globe,
  Languages,
  RefreshCw,
  Search,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { MODEL_CAPABILITY_LANGUAGES } from "@/lib/constants/languages";
import type { ModelFilter } from "./modelsView";

const CHIPS: readonly ModelFilter[] = ["all", "language", "fast"];

export interface ModelFilters {
  query: string;
  chip: ModelFilter;
  streaming: boolean;
  translation: boolean;
  /** Language code, or "all". */
  language: string;
}

interface ModelFilterBarProps {
  filters: ModelFilters;
  onChange: (next: ModelFilters) => void;
  isRescanning: boolean;
  isImporting: boolean;
  onRescan: () => void;
  onImport: () => void;
}

/** Search, quick chips, capability toggles and the language picker. */
export const ModelFilterBar: React.FC<ModelFilterBarProps> = ({
  filters,
  onChange,
  isRescanning,
  isImporting,
  onRescan,
  onImport,
}) => {
  const { t } = useTranslation();
  const set = (patch: Partial<ModelFilters>) =>
    onChange({ ...filters, ...patch });

  return (
    <div className="st-filterbar">
      <div className="st-search">
        <Search width={15} height={15} aria-hidden="true" />
        <input
          type="search"
          value={filters.query}
          onChange={(event) => set({ query: event.target.value })}
          placeholder={t("settings.models.searchPlaceholder")}
          aria-label={t("settings.models.searchPlaceholder")}
        />
      </div>
      <div
        className="st-chips"
        role="group"
        aria-label={t("settings.models.chips.label")}
      >
        {CHIPS.map((chip) => (
          <button
            key={chip}
            type="button"
            className="st-chip-btn"
            aria-pressed={filters.chip === chip}
            onClick={() => set({ chip })}
          >
            {t(`settings.models.chips.${chip}`)}
          </button>
        ))}
      </div>
      <div className="st-tools">
        <button
          type="button"
          className="st-icon-btn"
          onClick={onRescan}
          disabled={isRescanning}
          title={t("settings.models.rescan.tooltip")}
          aria-label={t("settings.models.rescan.tooltip")}
        >
          <RefreshCw
            width={15}
            height={15}
            className={isRescanning ? "animate-spin" : ""}
            aria-hidden="true"
          />
        </button>
        <button
          type="button"
          className="st-icon-btn"
          onClick={onImport}
          disabled={isImporting}
          title={t("settings.models.import.button")}
          aria-label={t("settings.models.import.button")}
        >
          <FolderInput width={15} height={15} aria-hidden="true" />
        </button>
        <button
          type="button"
          className="st-icon-btn"
          aria-pressed={filters.streaming}
          title={t("settings.models.filters.streaming")}
          aria-label={t("settings.models.filters.streaming")}
          onClick={() => set({ streaming: !filters.streaming })}
        >
          <AudioLines width={15} height={15} aria-hidden="true" />
        </button>
        <button
          type="button"
          className="st-icon-btn"
          aria-pressed={filters.translation}
          title={t("settings.models.filters.translation")}
          aria-label={t("settings.models.filters.translation")}
          onClick={() => set({ translation: !filters.translation })}
        >
          <Languages width={15} height={15} aria-hidden="true" />
        </button>
        <LanguageFilter
          value={filters.language}
          onChange={(language) => set({ language })}
        />
      </div>
    </div>
  );
};

/** Native select: full keyboard and screen-reader support, type-to-find. */
const LanguageFilter: React.FC<{
  value: string;
  onChange: (code: string) => void;
}> = ({ value, onChange }) => {
  const { t } = useTranslation();
  return (
    <label className="st-langfilter">
      <Globe width={14} height={14} aria-hidden="true" />
      <span className="sr-only">{t("settings.models.columns.languages")}</span>
      <select
        value={value}
        data-active={value !== "all"}
        onChange={(event) => onChange(event.target.value)}
      >
        <option value="all">{t("settings.models.filters.allLanguages")}</option>
        {MODEL_CAPABILITY_LANGUAGES.map((lang) => (
          <option key={lang.value} value={lang.value}>
            {lang.label}
          </option>
        ))}
      </select>
    </label>
  );
};
