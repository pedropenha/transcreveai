import React, { useState } from "react";
import { useTranslation } from "react-i18next";
import { commands, type CliAgentStatus } from "@/bindings";
import { useSettings } from "@/hooks/useSettings";
import { Alert } from "../../ui/Alert";
import { SettingContainer } from "../../ui/SettingContainer";
import type { DropdownOption } from "../../ui/Dropdown";
import { ProviderSelect } from "./ProviderSelect";
import { CliAgentFields } from "./CliAgentFields";

interface Props {
  options: DropdownOption[];
  statuses: Record<string, CliAgentStatus>;
  refreshStatuses: () => Promise<void>;
}

/** Configuration selection never changes the assistant or summary provider. */
export function CliAgentConfiguration({
  options,
  statuses,
  refreshStatuses,
}: Props) {
  const { t } = useTranslation();
  const { settings, refreshSettings, updatePostProcessModel, isUpdating } =
    useSettings();
  const cliOptions = options
    .filter((option) => option.value.startsWith("cli_agent/"))
    .map((option) => ({ ...option, disabled: false }));
  const [selected, setSelected] = useState("");
  const providerId = selected || cliOptions[0]?.value;
  const [updating, setUpdating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  if (!providerId) return null;
  const config = settings?.cli_agent_configs?.[providerId] ?? {
    enabled: true,
    binary_path: null,
    extra_args: [],
    timeout_secs: null,
  };
  return (
    <>
      <SettingContainer
        title={t("settings.postProcessing.cliAgent.configure.title")}
        description={t(
          "settings.postProcessing.cliAgent.configure.description",
        )}
        descriptionMode="inline"
        layout="horizontal"
        grouped
      >
        <ProviderSelect
          options={cliOptions}
          value={providerId}
          disabled={updating}
          onChange={(value) => {
            setSelected(value);
            setError(null);
          }}
        />
      </SettingContainer>
      {error && (
        <Alert variant="error" contained>
          {error}
        </Alert>
      )}
      <CliAgentFields
        key={providerId}
        status={statuses[providerId]}
        config={config}
        updating={updating}
        onConfigChange={async (patch) => {
          setUpdating(true);
          setError(null);
          try {
            const result = await commands.cliAgentUpdateConfig(providerId, {
              ...config,
              ...patch,
            });
            if (result.status === "error") {
              setError(t("settings.postProcessing.cliAgent.configure.error"));
              return;
            }
            await refreshSettings();
            await refreshStatuses();
          } catch {
            setError(t("settings.postProcessing.cliAgent.configure.error"));
          } finally {
            setUpdating(false);
          }
        }}
        model={settings?.post_process_models?.[providerId] ?? ""}
        onModelChange={(value) => {
          void updatePostProcessModel(providerId, value.trim());
        }}
        modelUpdating={isUpdating(`post_process_model:${providerId}`)}
      />
    </>
  );
}
