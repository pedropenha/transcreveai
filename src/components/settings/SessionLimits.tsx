import React from "react";
import { useTranslation } from "react-i18next";
import { useSettings } from "@/hooks/useSettings";
import { Slider } from "../ui/Slider";

export const SessionLimits: React.FC<{ grouped?: boolean }> = ({
  grouped = false,
}) => {
  const { t } = useTranslation();
  const { settings, updateSetting, resetSetting, isUpdating } = useSettings();
  return (
    <>
      <Slider
        value={settings?.max_dictation_minutes ?? 5}
        onChange={(value) => void updateSetting("max_dictation_minutes", value)}
        onReset={() => void resetSetting("max_dictation_minutes")}
        isResetting={isUpdating("max_dictation_minutes")}
        min={1}
        max={20}
        step={1}
        label={t("settingsHub.limits.duration.title")}
        description={t("settingsHub.limits.duration.description")}
        formatValue={(value) =>
          t("settingsHub.limits.duration.value", { value })
        }
        grouped={grouped}
      />
      <Slider
        value={settings?.session_queue_size ?? 5}
        onChange={(value) => void updateSetting("session_queue_size", value)}
        onReset={() => void resetSetting("session_queue_size")}
        isResetting={isUpdating("session_queue_size")}
        min={1}
        max={20}
        step={1}
        label={t("settingsHub.limits.queue.title")}
        description={t("settingsHub.limits.queue.description")}
        formatValue={(value) => String(value)}
        grouped={grouped}
      />
    </>
  );
};
