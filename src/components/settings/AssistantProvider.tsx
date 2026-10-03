import React from "react";
import { useTranslation } from "react-i18next";
import { commands } from "../../bindings";
import { useSettings } from "../../hooks/useSettings";
import { SettingContainer } from "../ui/SettingContainer";
import { ProviderSelect } from "./PostProcessingSettingsApi/ProviderSelect";
import { usePostProcessProviderState } from "./PostProcessingSettingsApi/usePostProcessProviderState";
import { assistantProviderOption } from "./assistantProviderOptions";
import { CliAgentConfiguration } from "./PostProcessingSettingsApi/CliAgentConfiguration";

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
    ...state.providerOptions.map((option) =>
      assistantProviderOption(option, state.cliAgents[option.value]),
    ),
  ];

  const onChange = (next: string) => {
    void commands
      .setAssistantProvider(next === "auto" ? null : next)
      .then(() => refreshSettings());
  };

  return (
    <>
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
      <CliAgentConfiguration
        options={state.providerOptions}
        statuses={state.cliAgents}
        refreshStatuses={state.refreshCliAgents}
      />
    </>
  );
};
