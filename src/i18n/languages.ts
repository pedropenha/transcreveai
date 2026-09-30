/**
 * Language metadata for supported locales.
 *
 * Transcreve.ai v1 ships only English and Brazilian Portuguese (ADR-0002).
 *
 * To add a new language:
 * 1. Create a new folder: src/i18n/locales/{code}/translation.json
 * 2. Add an entry here with the language code, English name, and native name
 * 3. Optionally add a priority (lower = higher in dropdown, no priority = alphabetical at end)
 * 4. For RTL languages, add direction: 'rtl'
 */
export const LANGUAGE_METADATA: Record<
  string,
  {
    name: string;
    nativeName: string;
    priority?: number;
    direction?: "ltr" | "rtl";
  }
> = {
  en: { name: "English", nativeName: "English", priority: 1 },
  "pt-BR": {
    name: "Portuguese (Brazil)",
    nativeName: "Português (Brasil)",
    priority: 2,
  },
};

/** UI language used when no preference is stored or a tag can't be resolved. */
export const FALLBACK_LANGUAGE = "en";

/**
 * Resolve an arbitrary BCP-47 tag ("pt", "pt_BR", "en-US", "de-DE") onto one of
 * the supported UI languages.
 *
 * Lookup order: exact match → same primary subtag ("pt" → "pt-BR",
 * "en-US" → "en") → null (caller falls back to English).
 */
export const resolveSupportedLanguage = (
  langCode: string | null | undefined,
  supportedCodes: readonly string[],
): string | null => {
  if (!langCode) return null;

  const normalized = langCode.toLowerCase().replace(/_/g, "-");
  const primary = normalized.split("-")[0];

  const exact = supportedCodes.find(
    (code) => code.toLowerCase() === normalized,
  );
  if (exact) return exact;

  return (
    supportedCodes.find(
      (code) => code.toLowerCase().split("-")[0] === primary,
    ) ?? null
  );
};
