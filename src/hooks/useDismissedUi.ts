import { useCallback } from "react";
import { useSettings } from "./useSettings";
import { useSettingsStore } from "../stores/settingsStore";

/**
 * Persisted "the user dismissed this" flags for Hub UI (banner, tip card,
 * setup checklist), stored in the backend `dismissed_ui` setting so they
 * survive restarts. Ids are owned by the callers.
 */
export function useDismissedUi() {
  const { settings, updateSetting } = useSettings();
  const dismissed = settings?.dismissed_ui ?? [];

  /**
   * Apply `change` to the *latest* stored list (read inside the callback, not
   * from render) so quick successive updates never overwrite each other.
   * Failures are logged; the store keeps its previous value.
   */
  const update = useCallback(
    async (change: (current: readonly string[]) => string[]) => {
      const current = useSettingsStore.getState().settings?.dismissed_ui ?? [];
      const next = change(current);
      if (
        next.length === current.length &&
        next.every((id, i) => id === current[i])
      ) {
        return;
      }
      try {
        await updateSetting("dismissed_ui", next);
      } catch (error) {
        console.warn("Failed to persist dismissed_ui:", error);
      }
    },
    [updateSetting],
  );

  const dismiss = useCallback(
    (id: string) =>
      update((current) =>
        current.includes(id) ? [...current] : [...current, id],
      ),
    [update],
  );

  const isDismissed = useCallback(
    (id: string) => dismissed.includes(id),
    [dismissed],
  );

  return { dismissed, isDismissed, dismiss, update };
}
