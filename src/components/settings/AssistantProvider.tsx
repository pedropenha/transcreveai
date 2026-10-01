import React from "react";
import { useTranslation } from "react-i18next";
import { commands } from "../../bindings";
import { useSettings } from "../../hooks/useSettings";
import { SettingContainer } from "../ui/SettingContainer";
import { ProviderSelect } from "./PostProcessingSettingsApi/ProviderSelect";
import { usePostProcessProviderState } from "./PostProcessingSettingsApi/usePostProcessProviderState";

/**
 * F012/T-091: which provider answers the assistant overlay. Reuses the
 * post-processing provider list (BYOK + `cli_agent/*` rows) — the fetch is
 * one `cli_agents_status` + one settings read. "Auto" resolves to the
 * configured provider or a detected CLI agent (`assistant_provider_id =
 * None` on the Rust side, FR-012-04).
 */
export const AssistantProvider: React.FC<{
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}> = ({ descriptionMode = "inline", grouped = false }) => {
  const { t } = useTranslation();
  const { getSetting, refreshSettings } = useSettings();
  const state = usePostProcessProviderState();

  const value = getSetting("assistant_provider_id") ?? "auto";
  const options = [
    { value: "auto", label: t("settingsHub.assistant.provider.auto") },
    // The assistant dropdown keeps *experimental* CLI agents selectable —
    // choosing one here is the user's explicit opt-in (FR-012-04), the
    // label still flags it experimental. Missing/disabled rows stay off.
    ...state.providerOptions.map((option) =>
      option.disabled && state.cliAgents[option.value]?.experimental
        ? { ...option, disabled: false }
        : option,
    ),
  ];

  const onChange = (next: string) => {
    void commands
      .setAssistantProvider(next === "auto" ? null : next)
      .then(() => refreshSettings());
  };

  return (
    <SettingContainer
      title={t("settingsHub.assistant.provider.title")}
      description={t("settingsHub.assistant.provider.description")}
      descriptionMode={descriptionMode}
      layout="horizontal"
      grouped={grouped}
    >
      <div className="flex items-center gap-2">
        <ProviderSelect options={options} value={value} onChange={onChange} />
      </div>
    </SettingContainer>
  );
};
