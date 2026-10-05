/**
 * Settings sub-navigation model (F010 FR-010-27): the categories and pages of
 * Configurações, plus the translation of every deep-link spelling (old flat
 * tab names, bare category names, full `category/page` paths) onto one page.
 * Pure so it is unit-tested without React.
 */

export const SETTINGS_CATEGORIES = [
  "usage",
  "transcription",
  "intelligence",
  "app",
] as const;
export type SettingsCategory = (typeof SETTINGS_CATEGORIES)[number];

export const SETTINGS_PAGES = [
  "usage/general",
  "usage/shortcuts",
  "usage/audio",
  "transcription/models",
  "transcription/languages",
  "transcription/api",
  "intelligence/summaries",
  "intelligence/assistant",
  "intelligence/connectors",
  "app/system",
  "app/privacy",
  "app/advanced",
] as const;
export type SettingsPage = (typeof SETTINGS_PAGES)[number];

export const DEFAULT_SETTINGS_PAGE: SettingsPage = "usage/general";

export interface SettingsPageEntry {
  id: SettingsPage;
  category: SettingsCategory;
  /** Last path segment, also the i18n key under `settingsHub.pages`. */
  slug: string;
}

export const SETTINGS_PAGE_ENTRIES: readonly SettingsPageEntry[] =
  SETTINGS_PAGES.map((id) => {
    const [category, slug] = id.split("/") as [SettingsCategory, string];
    return { id, category, slug };
  });

export function pagesOf(category: SettingsCategory): SettingsPageEntry[] {
  return SETTINGS_PAGE_ENTRIES.filter((entry) => entry.category === category);
}

export function categoryOf(page: SettingsPage): SettingsCategory {
  return page.split("/")[0] as SettingsCategory;
}

/** Spellings used before the sub-navigation existed. */
const LEGACY_TABS: Readonly<Record<string, SettingsPage>> = {
  general: "usage/general",
  models: "transcription/models",
  system: "app/system",
  privacy: "app/privacy",
  advanced: "app/advanced",
};

/** A bare category lands on its first page. */
const CATEGORY_LANDING: Readonly<Record<SettingsCategory, SettingsPage>> = {
  usage: "usage/general",
  transcription: "transcription/models",
  intelligence: "intelligence/summaries",
  app: "app/system",
};

const isPage = (value: string): value is SettingsPage =>
  (SETTINGS_PAGES as readonly string[]).includes(value);

const isCategory = (value: string): value is SettingsCategory =>
  (SETTINGS_CATEGORIES as readonly string[]).includes(value);

/**
 * Resolve any deep-link value (`"models"`, `"transcription"`,
 * `"transcription/models"`) to a page, or null when it names nothing.
 */
export function resolveSettingsPage(
  value: string | null | undefined,
): SettingsPage | null {
  const raw = value?.trim();
  if (!raw) return null;
  if (isPage(raw)) return raw;
  if (Object.prototype.hasOwnProperty.call(LEGACY_TABS, raw))
    return LEGACY_TABS[raw] ?? null;
  return isCategory(raw) ? CATEGORY_LANDING[raw] : null;
}
