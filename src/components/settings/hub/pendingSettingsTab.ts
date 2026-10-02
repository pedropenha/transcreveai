/**
 * F012 — deep-link landing tab for the Settings hub.
 *
 * `hub://navigate` payloads can carry a `settingsTab` (the assistant
 * panel's "Open settings" lands on the tab with the provider picker). The
 * event first navigates the sidebar, which is what mounts `SettingsHub` —
 * so the hub's own mount-time listener would miss every deep-link that
 * creates it. This module registers its listener at module evaluation,
 * long before React mounts anything in the hub window: a tab arriving
 * while no hub is subscribed is stashed, and `SettingsHub` drains the
 * stash on mount.
 */

import { listen } from "@tauri-apps/api/event";

/** Valid Settings hub tabs — kept in sync with `SettingsHub`'s TABS. */
export const SETTINGS_TABS = [
  "general",
  "models",
  "system",
  "privacy",
  "advanced",
] as const;
export type SettingsTab = (typeof SETTINGS_TABS)[number];

/** A stashed tab this old is assumed to come from an earlier interaction
 * and must not hijack an unrelated later mount. */
const PENDING_TAB_TTL_MS = 10_000;

let pending: { tab: SettingsTab; at: number } | null = null;
const subscribers = new Set<(tab: SettingsTab) => void>();

/** Route one `hub://navigate` payload — deliver to a live hub, or stash
 * while none is mounted. Exported for tests; the module listener calls it. */
export function handleNavigatePayload(payload: { settingsTab?: string }): void {
  const tab = payload.settingsTab;
  if (!tab || !(SETTINGS_TABS as readonly string[]).includes(tab)) return;
  const target = tab as SettingsTab;
  if (subscribers.size === 0) {
    pending = { tab: target, at: Date.now() };
  } else {
    subscribers.forEach((fn) => fn(target));
  }
}

try {
  // `void` + catch: in unit tests there is no Tauri runtime — the promise
  // rejects and the pure functions above are exercised directly.
  void listen<{ settingsTab?: string }>("hub://navigate", (event) =>
    handleNavigatePayload(event.payload),
  ).catch(() => {});
} catch {
  /* not a Tauri webview (tests) */
}

/** Read + clear a tab stashed while the hub was unmounted. Expired stashes
 * never hijack a later mount. */
export function takePendingSettingsTab(): SettingsTab | null {
  const stashed = pending;
  pending = null;
  if (stashed && Date.now() - stashed.at <= PENDING_TAB_TTL_MS) {
    return stashed.tab;
  }
  return null;
}

/** Live tab navigation while the hub is mounted — returns the unsubscribe. */
export function onSettingsTabNavigate(fn: (tab: SettingsTab) => void) {
  subscribers.add(fn);
  return () => {
    subscribers.delete(fn);
  };
}
