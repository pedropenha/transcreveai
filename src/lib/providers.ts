/**
 * STT provider helpers for the "Models & Providers" screen (FR-003-01..04).
 *
 * In v1 the only configurable providers are the installed local models. The
 * backend addresses them with the pseudo provider id `local_model:<model_id>`
 * in `dictation_provider_id` / `meeting_provider_id` / `fallback_provider_id`
 * (see `src-tauri/src/stt/selection.rs`); any other id is a `providers` table
 * row and lands with cloud providers in v1.1+.
 */

import type {
  AppSettings,
  EngineType,
  ModelInfo,
  ModelRecommendations,
  Suitability,
} from "@/bindings";

/** Prefix of the v1 pseudo provider id addressing a local model. */
export const LOCAL_MODEL_PROVIDER_PREFIX = "local_model:";

/** Build the provider id that addresses an installed local model. */
export const localModelProviderId = (modelId: string): string =>
  `${LOCAL_MODEL_PROVIDER_PREFIX}${modelId}`;

/** Decode `local_model:<id>` provider ids back to the model id. */
export const localModelIdFromProviderId = (
  providerId: string | null | undefined,
): string | null => {
  if (!providerId?.startsWith(LOCAL_MODEL_PROVIDER_PREFIX)) {
    return null;
  }
  const id = providerId.slice(LOCAL_MODEL_PROVIDER_PREFIX.length);
  return id === "" ? null : id;
};

/**
 * Which slot a provider selection fills on the backend command
 * `set_stt_provider` (FR-003-03).
 */
export type SttUsage = "dictation" | "meeting" | "fallback";

/**
 * Local provider family — the provider-type ids from F003 that
 * `LocalSttProvider` reports (`stt/local.rs::provider_id_for`).
 */
export type LocalProviderType =
  | "local_whisper"
  | "local_parakeet"
  | "local_onnx";

/** Map a model's engine onto its local provider family. */
export const providerTypeForEngine = (
  engine: EngineType,
): LocalProviderType => {
  switch (engine) {
    case "TranscribeCpp":
      return "local_whisper";
    case "Parakeet":
      return "local_parakeet";
    default:
      return "local_onnx";
  }
};

/** One row of the v1 providers list: a local engine family. */
export interface LocalProviderSummary {
  providerType: LocalProviderType;
  /** Catalog/custom models of this family seen by the registry. */
  totalModels: number;
  /** Models of this family already on disk. */
  downloadedModels: number;
  /** A model of this family is currently downloading/extracting. */
  downloading: boolean;
  /** Usage slots whose effective model belongs to this family. */
  usedFor: SttUsage[];
  /** Effective dictation/meeting/fallback model ids of this family. */
  usedModelIds: string[];
}

const PROVIDER_TYPE_ORDER: LocalProviderType[] = [
  "local_whisper",
  "local_parakeet",
  "local_onnx",
];

/**
 * Derive the v1 "configured providers" list from the model registry: one row
 * per local engine family that has at least one known model. `usageModelIds`
 * carries the *effective* model id per slot (after inheritance), or null.
 */
export const deriveLocalProviders = (
  models: ModelInfo[],
  usageModelIds: Record<SttUsage, string | null>,
  busyModelIds: ReadonlySet<string> = new Set(),
): LocalProviderSummary[] => {
  const usageEntries = (
    Object.entries(usageModelIds) as [SttUsage, string | null][]
  ).filter((entry): entry is [SttUsage, string] => entry[1] !== null);

  const summaries = new Map<LocalProviderType, LocalProviderSummary>();
  for (const model of models) {
    const providerType = providerTypeForEngine(model.engine_type);
    let summary = summaries.get(providerType);
    if (!summary) {
      summary = {
        providerType,
        totalModels: 0,
        downloadedModels: 0,
        downloading: false,
        usedFor: [],
        usedModelIds: [],
      };
      summaries.set(providerType, summary);
    }
    summary.totalModels += 1;
    if (model.is_downloaded) summary.downloadedModels += 1;
    if (model.is_downloading || busyModelIds.has(model.id)) {
      summary.downloading = true;
    }
    for (const [usage, modelId] of usageEntries) {
      if (modelId === model.id && !summary.usedFor.includes(usage)) {
        summary.usedFor.push(usage);
        summary.usedModelIds.push(model.id);
      }
    }
  }

  return PROVIDER_TYPE_ORDER.map((type) => summaries.get(type)).filter(
    (summary): summary is LocalProviderSummary => summary !== undefined,
  );
};

/** Effective dictation model: explicit provider id, else `selected_model`. */
export const effectiveDictationModelId = (
  settings: Pick<
    AppSettings,
    "dictation_provider_id" | "selected_model"
  > | null,
): string | null => {
  if (!settings) return null;
  return (
    localModelIdFromProviderId(settings.dictation_provider_id) ||
    settings.selected_model ||
    null
  );
};

/** Effective meeting model: explicit pick, else the dictation selection. */
export const effectiveMeetingModelId = (
  settings: Pick<
    AppSettings,
    "dictation_provider_id" | "meeting_provider_id" | "selected_model"
  > | null,
): string | null => {
  if (!settings) return null;
  return (
    localModelIdFromProviderId(settings.meeting_provider_id) ||
    effectiveDictationModelId(settings)
  );
};

/** Configured fallback model (only when it addresses a local model). */
export const effectiveFallbackModelId = (
  settings: Pick<AppSettings, "fallback_provider_id"> | null,
): string | null =>
  settings ? localModelIdFromProviderId(settings.fallback_provider_id) : null;

/** Suitability label for a model id from the hardware recommendations bundle. */
export const suitabilityForModel = (
  recommendations: ModelRecommendations | null,
  modelId: string,
): Suitability | undefined =>
  recommendations?.labels.find((entry) => entry.model_id === modelId)?.label;
