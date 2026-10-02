/**
 * Pure view helpers for Settings → Transcrição → Modelos (F003, F010
 * FR-010-27): score dots, size and language labels, acceleration summary.
 */

import type {
  HardwareReport,
  ModelInfo,
  TranscribeAcceleratorSetting,
} from "@/bindings";
import { supportsLanguageCode } from "@/lib/constants/languages";

export const SCORE_DOTS = 5;

/** 0..1 backend score → filled dots out of five (never 0 for a real score). */
export function scoreToDots(score: number): number {
  if (!Number.isFinite(score) || score <= 0) return 0;
  return Math.min(SCORE_DOTS, Math.max(1, Math.round(score * SCORE_DOTS)));
}

export type ModelFilter = "all" | "language" | "fast";

/** Table filter chips: all, supports the dictation language, fastest models. */
export function filterModels(
  models: readonly ModelInfo[],
  filter: ModelFilter,
  languageCode: string,
): ModelInfo[] {
  switch (filter) {
    case "language":
      return models.filter((m) =>
        supportsLanguageCode(m.supported_languages, languageCode),
      );
    case "fast":
      return models.filter((m) => m.speed_score >= 0.8);
    case "all":
      return [...models];
  }
}

export interface AccelerationSummary {
  kind: "gpu" | "cpu";
  /** GPU device name when one is known. */
  device: string | null;
}

/**
 * What transcribe.cpp will run on: the user's setting wins (cpu), otherwise a
 * detected GPU. `auto`/`gpu` without a detected device falls back to CPU.
 */
export function summarizeAcceleration(
  setting: TranscribeAcceleratorSetting | undefined,
  hardware: Pick<HardwareReport, "gpu_names"> | null | undefined,
): AccelerationSummary {
  const gpu = hardware?.gpu_names?.[0] ?? null;
  if (setting === "cpu" || gpu === null) return { kind: "cpu", device: null };
  return { kind: "gpu", device: gpu };
}
