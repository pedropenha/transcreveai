import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import type { CliAgentConfig, CliAgentStatus } from "@/bindings";

import { Alert } from "../../ui/Alert";
import { Input } from "../../ui/Input";
import { SettingContainer } from "../../ui/SettingContainer";
import { ToggleSwitch } from "../../ui/ToggleSwitch";

interface CliAgentFieldsProps {
  status: CliAgentStatus | undefined;
  config: CliAgentConfig;
  updating: boolean;
  onConfigChange: (patch: Partial<CliAgentConfig>) => void;
  model: string;
  onModelChange: (value: string) => void;
  modelUpdating: boolean;
}

/** Blur-committing text field — same pattern as BaseUrlField. */
const CommitInput: React.FC<{
  value: string;
  onCommit: (value: string) => void;
  disabled?: boolean;
  placeholder?: string;
  type?: string;
  className?: string;
}> = ({ value, onCommit, disabled, placeholder, type = "text", className }) => {
  const [local, setLocal] = useState(value);
  useEffect(() => {
    setLocal(value);
  }, [value]);
  return (
    <Input
      type={type}
      value={local}
      onChange={(event) => setLocal(event.target.value)}
      onBlur={() => {
        if (local.trim() !== value.trim()) onCommit(local);
      }}
      placeholder={placeholder}
      variant="compact"
      disabled={disabled}
      className={`flex-1 min-w-[320px] ${className ?? ""}`}
    />
  );
};

/**
 * F012 / FR-012-05: per-provider knobs for `cli_agent/*` providers —
 * enabled flag, binary path override, extra args, timeout — plus the
 * FR-012-02 detection banner. No API key field: auth is the CLI's own
 * sign-in (FR-012-03).
 */
export const CliAgentFields: React.FC<CliAgentFieldsProps> = React.memo(
  ({
    status,
    config,
    updating,
    onConfigChange,
    model,
    onModelChange,
    modelUpdating,
  }) => {
    const { t } = useTranslation();

    return (
      <>
        {status && !status.detected && (
          <Alert variant="error" contained>
            {t("settings.postProcessing.cliAgent.missingHint", {
              command: status.install_hint,
            })}
          </Alert>
        )}
        {status?.detected && (
          <Alert variant="success" contained>
            {t("settings.postProcessing.cliAgent.status.detectedDetail", {
              path: status.binary_name ?? status.binary,
            })}
          </Alert>
        )}

        <ToggleSwitch
          checked={config.enabled}
          onChange={(enabled) => onConfigChange({ enabled })}
          // Experimental adapters have no verified non-mutating mode — the
          // backend refuses to enable them, so keep the toggle off-limits.
          isUpdating={updating}
          disabled={status?.experimental}
          label={t("settings.postProcessing.cliAgent.enabled.title")}
          description={t(
            "settings.postProcessing.cliAgent.enabled.description",
          )}
          descriptionMode="tooltip"
          grouped={true}
        />

        <SettingContainer
          title={t("settings.postProcessing.cliAgent.binaryPath.title")}
          description={t(
            "settings.postProcessing.cliAgent.binaryPath.description",
          )}
          descriptionMode="tooltip"
          layout="horizontal"
          grouped={true}
        >
          <CommitInput
            value={config.binary_path ?? ""}
            onCommit={(value) =>
              onConfigChange({
                binary_path: value.trim() === "" ? null : value.trim(),
              })
            }
            disabled={updating}
            placeholder={t(
              "settings.postProcessing.cliAgent.binaryPath.placeholder",
              { binary: status?.binary ?? "" },
            )}
          />
        </SettingContainer>

        <SettingContainer
          title={t("settings.postProcessing.cliAgent.extraArgs.title")}
          description={t(
            "settings.postProcessing.cliAgent.extraArgs.description",
          )}
          descriptionMode="tooltip"
          layout="horizontal"
          grouped={true}
        >
          <CommitInput
            value={config.extra_args.join(" ")}
            onCommit={(value) =>
              onConfigChange({
                extra_args: value.trim().split(/\s+/).filter(Boolean),
              })
            }
            disabled={updating}
            placeholder={t(
              "settings.postProcessing.cliAgent.extraArgs.placeholder",
            )}
          />
        </SettingContainer>

        <SettingContainer
          title={t("settings.postProcessing.cliAgent.timeout.title")}
          description={t(
            "settings.postProcessing.cliAgent.timeout.description",
          )}
          descriptionMode="tooltip"
          layout="horizontal"
          grouped={true}
        >
          <CommitInput
            value={config.timeout_secs?.toString() ?? ""}
            onCommit={(value) => {
              const parsed = Number.parseInt(value.trim(), 10);
              onConfigChange({
                timeout_secs:
                  Number.isFinite(parsed) && parsed > 0 ? parsed : null,
              });
            }}
            disabled={updating}
            placeholder={t(
              "settings.postProcessing.cliAgent.timeout.placeholder",
            )}
            className="min-w-[120px]"
          />
        </SettingContainer>

        <SettingContainer
          title={t("settings.postProcessing.cliAgent.modelOverride.title")}
          description={t(
            "settings.postProcessing.cliAgent.modelOverride.description",
          )}
          descriptionMode="tooltip"
          layout="horizontal"
          grouped={true}
        >
          <CommitInput
            value={model}
            onCommit={onModelChange}
            disabled={modelUpdating}
            placeholder={t(
              "settings.postProcessing.cliAgent.modelOverride.placeholder",
            )}
          />
        </SettingContainer>
      </>
    );
  },
);

CliAgentFields.displayName = "CliAgentFields";
