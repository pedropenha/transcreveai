import type { ModelInfo } from "@/bindings";

/**
 * Whether the translated-dictation shortcut accepts this model. Same rule the
 * backend enforces (`validate_translation_model`): a TranscribeCpp model whose
 * catalog capability says it translates — so the models table can badge
 * exactly the models that will work.
 */
export const canTranslateToEnglish = (model: ModelInfo | undefined): boolean =>
  !!model &&
  model.engine_type === "TranscribeCpp" &&
  model.supports_translation;

/** Reuse the backend catalog capability rather than guessing by model name. */
export const getTranslationModels = (models: ModelInfo[]) =>
  models.filter(canTranslateToEnglish);

export function translationModelState(
  model: ModelInfo | undefined,
  downloading: boolean,
):
  | "unselected"
  | "incompatible"
  | "downloading"
  | "downloadRequired"
  | "ready" {
  if (!model) return "unselected";
  if (!canTranslateToEnglish(model)) return "incompatible";
  if (downloading || model.is_downloading) return "downloading";
  return model.is_downloaded ? "ready" : "downloadRequired";
}
