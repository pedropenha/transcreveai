import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useSettings } from "../../../hooks/useSettings";
import {
  commands,
  type CliAgentConfig,
  type CliAgentStatus,
  type PostProcessProvider,
} from "@/bindings";
import type { ModelOption } from "./types";
import type { DropdownOption } from "../../ui/Dropdown";

type PostProcessProviderState = {
  providerOptions: DropdownOption[];
  selectedProviderId: string;
  selectedProvider: PostProcessProvider | undefined;
  isCustomProvider: boolean;
  isAppleProvider: boolean;
  appleIntelligenceUnavailable: boolean;
  baseUrl: string;
  handleBaseUrlChange: (value: string) => void;
  isBaseUrlUpdating: boolean;
  // Masked vault hint ("••••1234") shown in the field — never the real key.
  apiKeyHint: string;
  handleApiKeyChange: (value: string) => void;
  isApiKeyUpdating: boolean;
  model: string;
  handleModelChange: (value: string) => void;
  modelOptions: ModelOption[];
  isModelUpdating: boolean;
  isFetchingModels: boolean;
  handleProviderSelect: (providerId: string) => void;
  handleModelSelect: (value: string) => void;
  handleModelCreate: (value: string) => void;
  handleRefreshModels: () => void;
  // cli_agent/* providers (F012): PATH detection status, per-provider config.
  isCliAgentProvider: boolean;
  cliAgentStatus: CliAgentStatus | undefined;
  // Full detection map (provider_id → status) — the assistant provider
  // picker uses it to keep experimental adapters selectable (explicit
  // opt-in) while the post-processing dropdown leaves them disabled.
  cliAgents: Record<string, CliAgentStatus>;
  cliAgentConfig: CliAgentConfig;
  isCliAgentUpdating: boolean;
  updateCliAgentConfig: (patch: Partial<CliAgentConfig>) => Promise<void>;
  refreshCliAgents: () => Promise<void>;
};

const APPLE_PROVIDER_ID = "apple_intelligence";
const CLI_AGENT_PREFIX = "cli_agent/";

const DEFAULT_CLI_AGENT_CONFIG: CliAgentConfig = {
  enabled: true,
  binary_path: null,
  extra_args: [],
  timeout_secs: null,
};

export const usePostProcessProviderState = (): PostProcessProviderState => {
  const { t } = useTranslation();
  const {
    settings,
    isUpdating,
    setPostProcessProvider,
    updatePostProcessBaseUrl,
    updatePostProcessApiKey,
    updatePostProcessModel,
    fetchPostProcessModels,
    postProcessModelOptions,
    apiKeyHints,
    refreshSettings,
  } = useSettings();

  // Settings are guaranteed to have providers after migration
  const providers = settings?.post_process_providers || [];

  // FR-012-02: PATH detection for `cli_agent/*` providers — pure file
  // checks on the backend, safe to poll on mount.
  const [cliAgents, setCliAgents] = useState<Record<string, CliAgentStatus>>(
    {},
  );
  const refreshCliAgents = useCallback(async () => {
    try {
      const result = await commands.cliAgentsStatus();
      if (result.status === "ok") {
        setCliAgents(
          Object.fromEntries(result.data.map((s) => [s.provider_id, s])),
        );
      }
    } catch (error) {
      console.error("Failed to detect CLI agents:", error);
    }
  }, []);
  useEffect(() => {
    void refreshCliAgents();
  }, [refreshCliAgents]);

  const selectedProviderId = useMemo(() => {
    return settings?.post_process_provider_id || providers[0]?.id || "openai";
  }, [providers, settings?.post_process_provider_id]);

  const selectedProvider = useMemo(() => {
    return (
      providers.find((provider) => provider.id === selectedProviderId) ||
      providers[0]
    );
  }, [providers, selectedProviderId]);

  const isAppleProvider = selectedProvider?.id === APPLE_PROVIDER_ID;
  const [appleIntelligenceUnavailable, setAppleIntelligenceUnavailable] =
    useState(false);

  // Use settings directly as single source of truth
  const baseUrl = selectedProvider?.base_url ?? "";
  // The real key is never sent to the frontend — only this masked hint.
  const apiKeyHint = apiKeyHints[selectedProviderId] ?? "";
  const model = settings?.post_process_models?.[selectedProviderId] ?? "";

  const providerOptions = useMemo<DropdownOption[]>(() => {
    return providers.map((provider) => {
      if (!provider.id.startsWith(CLI_AGENT_PREFIX)) {
        return { value: provider.id, label: provider.label };
      }
      // FR-012-02 / AC-012-02: absent CLI agents stay listed but disabled,
      // with the install command as the hint.
      const status = cliAgents[provider.id];
      // Experimental adapters can never run (the backend refuses to enable
      // them) — listed, but permanently disabled.
      if (status?.experimental) {
        return {
          value: provider.id,
          label: `${provider.label} — ${t("settings.postProcessing.cliAgent.status.experimental")}`,
          disabled: true,
        };
      }
      if (status && !status.detected) {
        return {
          value: provider.id,
          label: `${provider.label} — ${t("settings.postProcessing.cliAgent.status.missing")}`,
          description: t("settings.postProcessing.cliAgent.missingHint", {
            command: status.install_hint,
          }),
          disabled: true,
        };
      }
      if (status && !status.enabled) {
        // A user-disabled agent stays *selectable* — its Enable toggle lives
        // in the CliAgentFields that only render for the selected provider.
        return {
          value: provider.id,
          label: `${provider.label} — ${t("settings.postProcessing.cliAgent.status.disabled")}`,
        };
      }
      return {
        value: provider.id,
        label: provider.label,
        // While detection is still loading, leave the option enabled — a
        // spawned call fails cleanly if the binary turns out missing.
      };
    });
  }, [providers, cliAgents, t]);

  const handleProviderSelect = useCallback(
    async (providerId: string) => {
      // Clear error state on any selection attempt (allows dismissing the error)
      setAppleIntelligenceUnavailable(false);

      if (providerId === selectedProviderId) return;

      // Check Apple Intelligence availability before selecting
      if (providerId === APPLE_PROVIDER_ID) {
        const available = await commands.checkAppleIntelligenceAvailable();
        if (available.status === "ok" && !available.data) {
          setAppleIntelligenceUnavailable(true);
          // Don't return - still set the provider so dropdown shows the selection
          // The backend gracefully handles unavailable Apple Intelligence
        }
      }

      await setPostProcessProvider(providerId);

      // cli_agent/* providers have no model endpoint and no API key —
      // selecting them only flips `post_process_provider_id`.
      if (providerId.startsWith(CLI_AGENT_PREFIX)) return;

      // Auto-fetch available models for the new provider so the model dropdown
      // reflects what's actually valid. Without this, a stale model value from
      // a previous provider/base_url can persist and silently 404 at runtime.
      // Skip when the provider isn't configured yet (no API key / empty base URL)
      // to avoid unnecessary backend errors.
      if (providerId !== APPLE_PROVIDER_ID) {
        const provider = providers.find((p) => p.id === providerId);
        const hasBaseUrl = (provider?.base_url ?? "").trim() !== "";
        const hasApiKey = Boolean(apiKeyHints[providerId]);

        if (provider?.id === "custom" ? hasBaseUrl : hasApiKey) {
          void fetchPostProcessModels(providerId);
        }
      }
    },
    [
      selectedProviderId,
      setPostProcessProvider,
      fetchPostProcessModels,
      providers,
      apiKeyHints,
    ],
  );

  const handleBaseUrlChange = useCallback(
    (value: string) => {
      if (!selectedProvider || selectedProvider.id !== "custom") {
        return;
      }
      const trimmed = value.trim();
      if (trimmed && trimmed !== baseUrl) {
        void updatePostProcessBaseUrl(selectedProvider.id, trimmed);
      }
    },
    [selectedProvider, baseUrl, updatePostProcessBaseUrl],
  );

  const handleApiKeyChange = useCallback(
    (value: string) => {
      // Blurring the untouched mask is a no-op; an empty value clears the key
      // and anything else replaces it — all write-only via `secret_set`.
      const trimmed = value.trim();
      if (trimmed !== apiKeyHint) {
        void updatePostProcessApiKey(selectedProviderId, trimmed);
      }
    },
    [apiKeyHint, selectedProviderId, updatePostProcessApiKey],
  );

  const handleModelChange = useCallback(
    (value: string) => {
      const trimmed = value.trim();
      if (trimmed !== model) {
        void updatePostProcessModel(selectedProviderId, trimmed);
      }
    },
    [model, selectedProviderId, updatePostProcessModel],
  );

  const handleModelSelect = useCallback(
    (value: string) => {
      void updatePostProcessModel(selectedProviderId, value.trim());
    },
    [selectedProviderId, updatePostProcessModel],
  );

  const handleModelCreate = useCallback(
    (value: string) => {
      void updatePostProcessModel(selectedProviderId, value);
    },
    [selectedProviderId, updatePostProcessModel],
  );

  const handleRefreshModels = useCallback(() => {
    if (isAppleProvider) return;
    void fetchPostProcessModels(selectedProviderId);
  }, [fetchPostProcessModels, isAppleProvider, selectedProviderId]);

  const availableModelsRaw = postProcessModelOptions[selectedProviderId] || [];

  const modelOptions = useMemo<ModelOption[]>(() => {
    const seen = new Set<string>();
    const options: ModelOption[] = [];

    const upsert = (value: string | null | undefined) => {
      const trimmed = value?.trim();
      if (!trimmed || seen.has(trimmed)) return;
      seen.add(trimmed);
      options.push({ value: trimmed, label: trimmed });
    };

    // Add available models from API
    for (const candidate of availableModelsRaw) {
      upsert(candidate);
    }

    // Ensure current model is in the list
    upsert(model);

    return options;
  }, [availableModelsRaw, model]);

  const isBaseUrlUpdating = isUpdating(
    `post_process_base_url:${selectedProviderId}`,
  );
  const isApiKeyUpdating = isUpdating(
    `post_process_api_key:${selectedProviderId}`,
  );
  const isModelUpdating = isUpdating(
    `post_process_model:${selectedProviderId}`,
  );
  const isFetchingModels = isUpdating(
    `post_process_models_fetch:${selectedProviderId}`,
  );

  const isCustomProvider = selectedProvider?.id === "custom";
  const isCliAgentProvider = selectedProviderId.startsWith(CLI_AGENT_PREFIX);
  const cliAgentStatus = cliAgents[selectedProviderId];
  const cliAgentConfig: CliAgentConfig =
    settings?.cli_agent_configs?.[selectedProviderId] ??
    DEFAULT_CLI_AGENT_CONFIG;

  const updateCliAgentConfig = useCallback(
    async (patch: Partial<CliAgentConfig>) => {
      const current =
        settings?.cli_agent_configs?.[selectedProviderId] ??
        DEFAULT_CLI_AGENT_CONFIG;
      const result = await commands.cliAgentUpdateConfig(selectedProviderId, {
        ...current,
        ...patch,
      });
      if (result.status === "ok") {
        await refreshSettings();
        // A binary-path override can flip detection — re-poll.
        void refreshCliAgents();
      } else {
        console.error(
          "Failed to update CLI agent config:",
          result.error.message,
        );
      }
    },
    [settings, selectedProviderId, refreshSettings, refreshCliAgents],
  );

  // No automatic fetching - user must click refresh button

  return {
    providerOptions,
    selectedProviderId,
    selectedProvider,
    isCustomProvider,
    isAppleProvider,
    appleIntelligenceUnavailable,
    baseUrl,
    handleBaseUrlChange,
    isBaseUrlUpdating,
    apiKeyHint,
    handleApiKeyChange,
    isApiKeyUpdating,
    model,
    handleModelChange,
    modelOptions,
    isModelUpdating,
    isFetchingModels,
    handleProviderSelect,
    handleModelSelect,
    handleModelCreate,
    handleRefreshModels,
    isCliAgentProvider,
    cliAgentStatus,
    cliAgents,
    cliAgentConfig,
    isCliAgentUpdating: isUpdating(`cli_agent_config:${selectedProviderId}`),
    updateCliAgentConfig,
    refreshCliAgents,
  };
};
