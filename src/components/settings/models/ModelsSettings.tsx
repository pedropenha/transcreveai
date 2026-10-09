import React, { useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { ask, message, open } from "@tauri-apps/plugin-dialog";
import type { ModelCardStatus } from "@/components/onboarding";
import { useModelStore } from "@/stores/modelStore";
import { useSettingsStore } from "@/stores/settingsStore";
import { ProvidersSection } from "./ProvidersSection";
import { UsageSelectors } from "./UsageSelectors";
import {
  deriveLocalProviders,
  effectiveDictationModelId,
  effectiveFallbackModelId,
  effectiveMeetingModelId,
  gpuAcceleratedForModel,
  suitabilityForModel,
  type SttUsage,
} from "@/lib/providers";
import { supportsLanguageCode } from "@/lib/constants/languages";
import type { ModelInfo } from "@/bindings";
import { ModelFilterBar, type ModelFilters } from "./ModelFilterBar";
import { ModelHero } from "./ModelHero";
import { ModelTable, type ModelRowState } from "./ModelTable";
import { filterModels, summarizeAcceleration } from "./modelsView";

// Legacy models are the blob (Url-sourced) .bin/ONNX downloads, superseded by
// the catalog GGUFs. They stay runnable when already on disk, but we no longer
// advertise the download.
const isLegacyModel = (model: ModelInfo): boolean =>
  typeof model.source === "object" && "Url" in model.source;

const INITIAL_FILTERS: ModelFilters = {
  query: "",
  chip: "all",
  streaming: false,
  translation: false,
  language: "all",
};

export const ModelsSettings: React.FC = () => {
  const { t } = useTranslation();
  const [switchingModelId, setSwitchingModelId] = useState<string | null>(null);
  const [filters, setFilters] = useState<ModelFilters>(INITIAL_FILTERS);
  const {
    models,
    currentModel,
    downloadingModels,
    downloadProgress,
    downloadStats,
    verifyingModels,
    extractingModels,
    loading,
    isRescanning,
    recommendations,
    downloadModel,
    cancelDownload,
    selectModel,
    deleteModel,
    rescanLocalModels,
    importModel,
    loadRecommendations,
  } = useModelStore();
  const settings = useSettingsStore((s) => s.settings);
  const [isImporting, setIsImporting] = useState(false);

  // Hardware suitability labels are advisory — fetch once, never block on it.
  useEffect(() => {
    void loadRecommendations();
  }, [loadRecommendations]);

  // Effective per-usage picks (meeting inherits dictation when unset).
  const usageModelIds = useMemo<Record<SttUsage, string | null>>(
    () => ({
      dictation: effectiveDictationModelId(settings),
      meeting: effectiveMeetingModelId(settings),
      fallback: effectiveFallbackModelId(settings),
    }),
    [settings],
  );

  // model.id -> usage badges shown on its card.
  const usagesByModelId = useMemo(() => {
    const map = new Map<string, SttUsage[]>();
    for (const [usage, modelId] of Object.entries(usageModelIds) as [
      SttUsage,
      string | null,
    ][]) {
      if (!modelId) continue;
      const list = map.get(modelId) ?? [];
      list.push(usage);
      map.set(modelId, list);
    }
    return map;
  }, [usageModelIds]);

  const providers = useMemo(
    () =>
      deriveLocalProviders(
        models,
        usageModelIds,
        new Set([
          ...Object.keys(downloadingModels),
          ...Object.keys(extractingModels),
        ]),
      ),
    [models, usageModelIds, downloadingModels, extractingModels],
  );

  const getModelStatus = (modelId: string): ModelCardStatus => {
    if (modelId in extractingModels) {
      return "extracting";
    }
    if (modelId in verifyingModels) {
      return "verifying";
    }
    if (modelId in downloadingModels) {
      return "downloading";
    }
    if (switchingModelId === modelId) {
      return "switching";
    }
    const model = models.find((m: ModelInfo) => m.id === modelId);
    // A stale persisted selection must never make a missing model look Active.
    // Catalog models without files should offer their recovery action instead.
    if (!model?.is_downloaded) {
      return "downloadable";
    }
    if (modelId === currentModel) {
      return "active";
    }
    return "available";
  };

  const getDownloadProgress = (modelId: string): number | undefined => {
    const progress = downloadProgress[modelId];
    return progress?.percentage;
  };

  const getDownloadSpeed = (modelId: string): number | undefined => {
    const stats = downloadStats[modelId];
    return stats?.speed;
  };

  const handleModelSelect = async (modelId: string) => {
    setSwitchingModelId(modelId);
    try {
      await selectModel(modelId);
    } catch (err) {
      console.error(`Failed to select model ${modelId}:`, err);
      toast.error(t("settings.models.actionError"));
    } finally {
      setSwitchingModelId(null);
    }
  };

  const handleModelDownload = async (modelId: string) => {
    try {
      await downloadModel(modelId);
    } catch (err) {
      console.error(`Failed to download model ${modelId}:`, err);
      toast.error(t("settings.models.actionError"));
    }
  };

  const handleModelDelete = async (modelId: string) => {
    const model = models.find((m: ModelInfo) => m.id === modelId);
    const modelName = model?.name || modelId;
    const isActive = modelId === currentModel;

    const confirmed = await ask(
      isActive
        ? t("settings.models.deleteActiveConfirm", { modelName })
        : t("settings.models.deleteConfirm", { modelName }),
      {
        title: t("settings.models.deleteTitle"),
        kind: "warning",
      },
    );

    if (confirmed) {
      try {
        await deleteModel(modelId);
      } catch (err) {
        console.error(`Failed to delete model ${modelId}:`, err);
      }
    }
  };

  const handleModelCancel = async (modelId: string) => {
    try {
      await cancelDownload(modelId);
    } catch (err) {
      console.error(`Failed to cancel download for ${modelId}:`, err);
    }
  };

  // FR-003-07: pick a .gguf/.bin on disk; the backend format-checks it,
  // verifies disk space, and returns the SHA-256 shown to the user.
  const handleImportModel = async () => {
    try {
      const selected = await open({
        multiple: false,
        directory: false,
        filters: [
          {
            name: t("settings.models.import.filterName"),
            extensions: ["gguf", "bin"],
          },
        ],
      });
      if (typeof selected !== "string" || selected === "") return;
      setIsImporting(true);
      const imported = await importModel(selected);
      if (imported) {
        await message(
          t("settings.models.import.success", {
            modelName: imported.model.name,
            sha256: imported.sha256,
          }),
          {
            title: t("settings.models.import.title"),
            kind: "info",
          },
        );
      } else {
        await message(
          useModelStore.getState().error ?? t("settings.models.actionError"),
          {
            title: t("settings.models.import.title"),
            kind: "error",
          },
        );
      }
    } catch (err) {
      console.error("Failed to import model:", err);
    } finally {
      setIsImporting(false);
    }
  };

  // Search + capability toggles + language + quick chip, then split into the
  // active model (hero) and everything else (table).
  const dictationLanguage = settings?.selected_language ?? "auto";
  const visibleModels = useMemo(() => {
    const q = filters.query.trim().toLowerCase();
    const base = models.filter((model: ModelInfo) => {
      // Hide deprecated legacy (.bin/ONNX) downloads unless already on disk.
      if (isLegacyModel(model) && !model.is_downloaded) return false;
      if (
        filters.language !== "all" &&
        !supportsLanguageCode(model.supported_languages, filters.language)
      ) {
        return false;
      }
      if (filters.streaming && !model.supports_streaming) return false;
      if (filters.translation && !model.supports_translation) return false;
      if (q) {
        const haystack = `${model.name} ${model.description}`.toLowerCase();
        if (!haystack.includes(q)) return false;
      }
      return true;
    });
    return filterModels(base, filters.chip, dictationLanguage);
  }, [models, filters, dictationLanguage]);

  const activeModel = useMemo(
    () => models.find((m) => m.id === currentModel && m.is_downloaded) ?? null,
    [models, currentModel],
  );

  // Downloaded (incl. custom / in progress) first, then the downloadable rest.
  const isOwned = (m: ModelInfo) =>
    m.is_custom ||
    m.is_downloaded ||
    m.id in downloadingModels ||
    m.id in extractingModels;
  const others = visibleModels.filter((m) => m.id !== activeModel?.id);
  const rows: ModelRowState[] = [
    ...others.filter((m) => isOwned(m) && !m.is_custom),
    ...others.filter((m) => m.is_custom),
    ...others.filter((m) => !isOwned(m)),
  ].map((model) => ({
    model,
    status: getModelStatus(model.id),
    progress: getDownloadProgress(model.id),
    speed: getDownloadSpeed(model.id),
    suitability: suitabilityForModel(recommendations, model.id),
    gpuAccelerated: gpuAcceleratedForModel(recommendations, model.id),
    usages: usagesByModelId.get(model.id),
  }));

  const acceleration = summarizeAcceleration(
    settings?.transcribe_accelerator,
    recommendations?.hardware,
  );

  if (loading) {
    return (
      <div className="max-w-3xl w-full mx-auto">
        <div className="flex items-center justify-center py-16">
          <div className="w-8 h-8 border-2 border-logo-primary border-t-transparent rounded-full animate-spin" />
        </div>
      </div>
    );
  }

  return (
    <div className="st-page">
      <div
        className="st-seg"
        role="group"
        aria-label={t("settings.models.where.label")}
      >
        <button type="button" aria-pressed="true">
          {t("settings.models.where.local")}
        </button>
        <button type="button" aria-pressed="false" disabled>
          {t("settings.models.where.api")}
          <span className="st-chip st-chip-sm" data-tone="outline">
            {t("settingsHub.soon")}
          </span>
        </button>
      </div>

      <section className="st-section" aria-labelledby="models-inuse">
        <h2 id="models-inuse" className="caps">
          {t("settings.models.inUse")}
        </h2>
        <ModelHero model={activeModel} acceleration={acceleration} />
      </section>

      <section className="st-section" aria-labelledby="models-catalog">
        <h2 id="models-catalog" className="caps">
          {t("settings.models.catalog")}
        </h2>
        <ModelFilterBar
          filters={filters}
          onChange={setFilters}
          isRescanning={isRescanning}
          isImporting={isImporting}
          onRescan={() => void rescanLocalModels()}
          onImport={() => void handleImportModel()}
        />
        {rows.length > 0 ? (
          <ModelTable
            rows={rows}
            caption={t("settings.models.catalog")}
            onSelect={handleModelSelect}
            onDownload={handleModelDownload}
            onDelete={handleModelDelete}
            onCancel={handleModelCancel}
          />
        ) : (
          <p className="st-empty">{t("settings.models.noModelsMatch")}</p>
        )}
        <p className="st-note">{t("settings.models.catalogNote")}</p>
      </section>

      {/* v1 providers = the local engine families behind the catalog
          (FR-003-01); cloud providers return in v1.1 (T-014). */}
      <ProvidersSection providers={providers} />

      {/* Per-usage model picks: dictation / meeting / fallback (FR-003-03). */}
      <UsageSelectors />
    </div>
  );
};
