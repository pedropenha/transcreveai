import React, { useMemo } from "react";
import { Check, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { useSettings } from "../../hooks/useSettings";
import { useDismissedUi } from "../../hooks/useDismissedUi";
import { useModelStore } from "../../stores/modelStore";
import type { Navigation } from "./navModel";
import {
  SETUP_CHECKLIST_ID,
  buildSetupItems,
  isChecklistVisible,
  markSetupItemDone,
  setupProgress,
  type SetupItem,
} from "./setupItems";

interface SetupChecklistProps {
  onNavigate: (navigation: Navigation) => void;
}

/** Fixed card at the foot of the rail until finished or dismissed. */
export const SetupChecklist: React.FC<SetupChecklistProps> = ({
  onNavigate,
}) => {
  const { t } = useTranslation();
  const { settings } = useSettings();
  const { dismissed, dismiss, update } = useDismissedUi();
  const hasLocalModel = useModelStore((state) =>
    state.models.some((model) => model.is_downloaded),
  );
  const hasSummaryProvider = Boolean(settings?.meeting_provider_id);

  const items = useMemo(
    () => buildSetupItems({ hasLocalModel, hasSummaryProvider, dismissed }),
    [hasLocalModel, hasSummaryProvider, dismissed],
  );

  // Wait for settings so a dismissed card never flashes on startup.
  if (!settings || !isChecklistVisible(items, dismissed)) return null;

  const { done, total } = setupProgress(items);

  const open = (item: SetupItem) => {
    if (item.manual && !item.done) {
      void update((current) => markSetupItemDone(current, item.id));
    }
    onNavigate({ section: "settings", settingsTab: item.settingsTab });
  };

  return (
    <section
      className="setup-card"
      aria-label={t("setupChecklist.title")}
      data-testid="setup-checklist"
    >
      <header className="setup-card-head">
        <h2>{t("setupChecklist.title")}</h2>
        <button
          type="button"
          className="icon-button"
          aria-label={t("setupChecklist.dismiss")}
          onClick={() => void dismiss(SETUP_CHECKLIST_ID)}
        >
          <X width={14} height={14} aria-hidden="true" />
        </button>
      </header>
      <div
        className="setup-progress"
        role="progressbar"
        aria-valuemin={0}
        aria-valuemax={total}
        aria-valuenow={done}
        aria-label={t("setupChecklist.progress", { done, total })}
      >
        <i style={{ width: `${(done / total) * 100}%` }} />
      </div>
      <ul>
        {items.map((item) => (
          <li key={item.id}>
            <button
              type="button"
              className="setup-item"
              data-done={item.done}
              onClick={() => open(item)}
            >
              <span className="setup-check" aria-hidden="true">
                {item.done ? (
                  <Check width={10} height={10} strokeWidth={3.5} />
                ) : null}
              </span>
              <span>{t(item.labelKey)}</span>
            </button>
          </li>
        ))}
      </ul>
    </section>
  );
};
