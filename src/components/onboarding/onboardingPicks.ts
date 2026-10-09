import type { ModelInfo } from "@/bindings";

/**
 * Featured onboarding picks per UI language: model-id fragments matched
 * against `ModelInfo.id` (`"{repo_id}/{filename}"`). The catalog's editorial
 * `recommended` set alone would front an English-only model to pt-BR
 * installs, so a first-run user sees the model we believe fits their
 * language before it.
 */
const LANGUAGE_FEATURED: Record<string, readonly string[]> = {
  // Parakeet TDT 0.6B v3 covers 25 European languages including Portuguese —
  // the English-only Parakeet Unified is a poor first-run pick for pt-BR.
  "pt-BR": ["parakeet-tdt-0.6b-v3"],
};

/**
 * The featured ("top pick") models for the download step: language picks
 * first, then the catalog's recommended set, deduplicated, capped at two.
 * `language` is an already-resolved supported code (`"en"`/`"pt-BR"`).
 */
export const featuredModels = (
  downloadable: ModelInfo[],
  language: string | null,
): ModelInfo[] => {
  const picks = language ? (LANGUAGE_FEATURED[language] ?? []) : [];
  const languageFeatured = picks.flatMap((pick) =>
    downloadable.filter((m) => m.id.includes(pick)),
  );
  const recommended = downloadable.filter((m) => m.is_recommended);
  const seen = new Set<string>();
  return [...languageFeatured, ...recommended]
    .filter((m) => (seen.has(m.id) ? false : (seen.add(m.id), true)))
    .slice(0, 2);
};
