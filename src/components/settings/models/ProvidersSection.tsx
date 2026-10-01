import React from "react";
import { useTranslation } from "react-i18next";
import { AudioLines, Cpu, Loader2, Mic } from "lucide-react";
import Badge from "@/components/ui/Badge";
import { SettingsGroup } from "@/components/ui/SettingsGroup";
import type { LocalProviderSummary } from "@/lib/providers";

const PROVIDER_ICONS: Record<
  LocalProviderSummary["providerType"],
  React.ComponentType<{ className?: string }>
> = {
  local_whisper: Mic,
  local_parakeet: AudioLines,
  local_onnx: Cpu,
};

interface ProvidersSectionProps {
  providers: LocalProviderSummary[];
}

/**
 * "Providers" block of the Models & Providers screen (FR-003-01). v1 lists
 * only the local engine families — cloud providers return in v1.1 (T-014),
 * which the note under the list states explicitly.
 */
export const ProvidersSection: React.FC<ProvidersSectionProps> = ({
  providers,
}) => {
  const { t } = useTranslation();

  return (
    <SettingsGroup title={t("settings.providers.title")}>
      {providers.map((provider) => {
        const Icon = PROVIDER_ICONS[provider.providerType];
        return (
          <div
            key={provider.providerType}
            className="flex items-center gap-3 px-4 py-3"
          >
            <span className="flex items-center justify-center w-9 h-9 rounded-lg bg-mid-gray/10 shrink-0">
              {provider.downloading ? (
                <Loader2 className="w-4.5 h-4.5 animate-spin text-logo-primary" />
              ) : (
                <Icon className="w-4.5 h-4.5 text-text/70" />
              )}
            </span>
            <div className="flex-1 min-w-0">
              <div className="flex items-center gap-2 flex-wrap">
                <h3 className="text-sm font-medium">
                  {t(`settings.providers.types.${provider.providerType}`)}
                </h3>
                <Badge variant="secondary">
                  {t("settings.providers.local")}
                </Badge>
                {provider.usedFor.map((usage) => (
                  <Badge key={usage} variant="primary">
                    {t(`settings.models.usage.${usage}`)}
                  </Badge>
                ))}
              </div>
              <p className="text-xs text-text/50 mt-0.5">
                {provider.downloading
                  ? t("settings.providers.downloadingModels", {
                      downloaded: provider.downloadedModels,
                      total: provider.totalModels,
                    })
                  : t("settings.providers.modelCount", {
                      downloaded: provider.downloadedModels,
                      total: provider.totalModels,
                    })}
              </p>
            </div>
          </div>
        );
      })}
      <p className="px-4 py-2.5 text-xs text-text/50 border-t border-mid-gray/20">
        {t("settings.providers.localOnlyNote")}
      </p>
    </SettingsGroup>
  );
};
