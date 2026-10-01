import React, { useMemo } from "react";
import { useTranslation } from "react-i18next";
import { Dropdown, type DropdownOption } from "@/components/ui/Dropdown";
import { SettingContainer } from "@/components/ui/SettingContainer";
import { SettingsGroup } from "@/components/ui/SettingsGroup";
import { getTranslatedModelName } from "@/lib/utils/modelTranslation";
import {
  effectiveDictationModelId,
  localModelIdFromProviderId,
  type SttUsage,
} from "@/lib/providers";
import { useModelStore } from "@/stores/modelStore";
import { useSettingsStore } from "@/stores/settingsStore";

// Sentinel option values for the "unset" entries of meeting/fallback.
const INHERIT = "__inherit__";
const NONE = "__none__";

/**
 * Per-usage model pickers (FR-003-03). v1 offers local models only; the
 * backend persists them as `local_model:<id>` provider ids in
 * `dictation_provider_id` / `meeting_provider_id` / `fallback_provider_id`.
 */
export const UsageSelectors: React.FC = () => {
  const { t } = useTranslation();
  const settings = useSettingsStore((s) => s.settings);
  const { models, setProviderForUsage } = useModelStore();

  const downloadedModels = useMemo(
    () => models.filter((m) => m.is_downloaded),
    [models],
  );

  const modelOptions = useMemo<DropdownOption[]>(
    () =>
      downloadedModels.map((m) => ({
        value: m.id,
        label: getTranslatedModelName(m, t),
      })),
    [downloadedModels, t],
  );

  const effectiveDictation = effectiveDictationModelId(settings);
  // The stored pick (null = inherit) is what the dropdown controls show.
  const meetingPick =
    localModelIdFromProviderId(settings?.meeting_provider_id) ?? INHERIT;
  const fallbackPick =
    localModelIdFromProviderId(settings?.fallback_provider_id) ?? NONE;

  const dictationModelName = useMemo(() => {
    const model = downloadedModels.find((m) => m.id === effectiveDictation);
    return model ? getTranslatedModelName(model, t) : null;
  }, [downloadedModels, effectiveDictation, t]);

  const handleUsage = (usage: SttUsage, value: string) => {
    const modelId =
      usage === "meeting" && value === INHERIT
        ? null
        : usage === "fallback" && value === NONE
          ? null
          : value;
    void setProviderForUsage(usage, modelId);
  };

  const meetingOptions: DropdownOption[] = [
    {
      value: INHERIT,
      label: t("settings.models.usage.sameAsDictation"),
      description: dictationModelName ?? undefined,
    },
    ...modelOptions,
  ];
  const fallbackOptions: DropdownOption[] = [
    { value: NONE, label: t("settings.models.usage.none") },
    ...modelOptions,
  ];

  return (
    <SettingsGroup
      title={t("settings.models.usage.title")}
      description={t("settings.models.usage.description")}
    >
      <SettingContainer
        title={t("settings.models.usage.dictation")}
        description={t("settings.models.usage.dictationDescription")}
        grouped
      >
        <Dropdown
          options={modelOptions}
          selectedValue={effectiveDictation}
          onSelect={(value) => handleUsage("dictation", value)}
          disabled={downloadedModels.length === 0}
          placeholder={t("settings.models.usage.noModel")}
        />
      </SettingContainer>
      <SettingContainer
        title={t("settings.models.usage.meeting")}
        description={t("settings.models.usage.meetingDescription", {
          modelName: dictationModelName ?? "",
        })}
        grouped
      >
        <Dropdown
          options={meetingOptions}
          selectedValue={meetingPick}
          onSelect={(value) => handleUsage("meeting", value)}
          disabled={downloadedModels.length === 0}
        />
      </SettingContainer>
      <SettingContainer
        title={t("settings.models.usage.fallback")}
        description={t("settings.models.usage.fallbackDescription")}
        grouped
      >
        <Dropdown
          options={fallbackOptions}
          selectedValue={fallbackPick}
          onSelect={(value) => handleUsage("fallback", value)}
          disabled={downloadedModels.length === 0}
        />
      </SettingContainer>
    </SettingsGroup>
  );
};
