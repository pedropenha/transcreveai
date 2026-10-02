/**
 * "Configurar Notetaker" checklist (F010). Items either derive from real state
 * (a downloaded model, a configured summary provider) or are completed by the
 * user visiting them; completion and dismissal persist in `dismissed_ui`.
 */

export const SETUP_CHECKLIST_ID = "setup_checklist";
const MANUAL_PREFIX = "setup:";

export type SetupItemId =
  | "model"
  | "microphone"
  | "system_audio"
  | "summary_key";

export interface SetupItem {
  id: SetupItemId;
  labelKey: string;
  done: boolean;
  /** Settings tab the item deep-links to. */
  settingsTab: string;
  /** Derived items complete on their own; manual ones when visited. */
  manual: boolean;
}

export interface SetupState {
  hasLocalModel: boolean;
  hasSummaryProvider: boolean;
  dismissed: readonly string[];
}

export function buildSetupItems(state: SetupState): SetupItem[] {
  const visited = (id: SetupItemId) =>
    state.dismissed.includes(`${MANUAL_PREFIX}${id}`);
  return [
    {
      id: "model",
      labelKey: "setupChecklist.items.model",
      done: state.hasLocalModel,
      settingsTab: "transcription/models",
      manual: false,
    },
    {
      id: "microphone",
      labelKey: "setupChecklist.items.microphone",
      done: visited("microphone"),
      settingsTab: "usage/audio",
      manual: true,
    },
    {
      id: "system_audio",
      labelKey: "setupChecklist.items.systemAudio",
      done: visited("system_audio"),
      settingsTab: "app/privacy",
      manual: true,
    },
    {
      id: "summary_key",
      labelKey: "setupChecklist.items.summaryKey",
      done: state.hasSummaryProvider,
      settingsTab: "intelligence/summaries",
      manual: false,
    },
  ];
}

export function setupProgress(items: readonly SetupItem[]) {
  return {
    done: items.filter((item) => item.done).length,
    total: items.length,
  };
}

/** Immutable: returns a new list with the manual completion id appended. */
export function markSetupItemDone(
  dismissed: readonly string[],
  id: SetupItemId,
): string[] {
  const entry = `${MANUAL_PREFIX}${id}`;
  return dismissed.includes(entry) ? [...dismissed] : [...dismissed, entry];
}

export function isChecklistVisible(
  items: readonly SetupItem[],
  dismissed: readonly string[],
): boolean {
  if (dismissed.includes(SETUP_CHECKLIST_ID)) return false;
  const { done, total } = setupProgress(items);
  return done < total;
}
