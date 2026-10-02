/**
 * Hub navigation model (F010): which sections exist, how the rail groups them,
 * keyboard shortcuts and legacy deep-link redirects. Pure so it is unit-tested
 * without React; `Sidebar` and `App` consume it.
 */

export const HUB_SECTIONS = [
  "home",
  "meetings",
  "dictionary",
  "settings",
  "help",
] as const;
export type HubSection = (typeof HUB_SECTIONS)[number];

export type RailGroup = "main" | "customize" | "footer";

export interface RailEntry {
  id: string;
  group: RailGroup;
  labelKey: string;
  /** "soon" entries (v1.1+) are shown disabled with a tag and never navigate. */
  availability: "available" | "soon";
  /** Target section; absent for entries that are not navigable yet. */
  section?: HubSection;
}

export const RAIL_ENTRIES: readonly RailEntry[] = [
  {
    id: "home",
    group: "main",
    labelKey: "sidebar.home",
    availability: "available",
    section: "home",
  },
  {
    id: "meetings",
    group: "main",
    labelKey: "sidebar.meetings",
    availability: "available",
    section: "meetings",
  },
  {
    id: "notes",
    group: "main",
    labelKey: "sidebar.notes",
    availability: "soon",
  },
  {
    id: "dictionary",
    group: "customize",
    labelKey: "sidebar.dictionary",
    availability: "available",
    section: "dictionary",
  },
  {
    id: "assistant",
    group: "customize",
    labelKey: "sidebar.assistant",
    availability: "soon",
  },
  {
    id: "settings",
    group: "footer",
    labelKey: "sidebar.settings",
    availability: "available",
    section: "settings",
  },
  {
    id: "help",
    group: "footer",
    labelKey: "sidebar.help",
    availability: "available",
    section: "help",
  },
];

/** Ctrl/Cmd+1..5 follow the rail order, skipping v1.1 entries. */
const SHORTCUT_SECTIONS: readonly HubSection[] = RAIL_ENTRIES.flatMap(
  (entry) => (entry.section ? [entry.section] : []),
);

export interface ShortcutKeyEvent {
  key: string;
  ctrlKey: boolean;
  metaKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
}

/** Section a keyboard event navigates to, or null when it is not ours. */
export function sectionForShortcut(event: ShortcutKeyEvent): HubSection | null {
  if (!(event.ctrlKey || event.metaKey) || event.shiftKey || event.altKey) {
    return null;
  }
  if (event.key === ",") return "settings";
  if (!/^[1-9]$/.test(event.key)) return null;
  return SHORTCUT_SECTIONS[Number(event.key) - 1] ?? null;
}

/** Settings tab that holds the meeting-summary provider (Notetaker deep link). */
export const SUMMARY_SETTINGS_TAB = "general";

export interface NavigatePayload {
  section?: string;
  settingsTab?: string;
}

export interface Navigation {
  section: HubSection | null;
  settingsTab?: string;
}

/** Sections that moved into Settings: old deep links keep working. */
const LEGACY_SETTINGS_TAB: Readonly<Record<string, string>> = {
  models: "models",
};

const isHubSection = (value: string): value is HubSection =>
  (HUB_SECTIONS as readonly string[]).includes(value);

/**
 * Resolve a `hub://navigate` payload. `models` (removed from the rail) lands on
 * Settings → Models; an explicit `settingsTab` always wins over that default.
 */
export function resolveNavigation(payload: NavigatePayload): Navigation {
  const { section, settingsTab } = payload;
  const withTab = (nav: Navigation): Navigation =>
    settingsTab !== undefined ? { ...nav, settingsTab } : nav;

  if (section === undefined) return withTab({ section: null });
  const legacyTab = LEGACY_SETTINGS_TAB[section];
  if (legacyTab !== undefined) {
    return { section: "settings", settingsTab: settingsTab ?? legacyTab };
  }
  if (!isHubSection(section)) return { section: null };
  return withTab({ section });
}
