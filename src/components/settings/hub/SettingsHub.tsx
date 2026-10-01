import React, { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { useTranslation } from "react-i18next";
import { GeneralSettings } from "../general/GeneralSettings";
import { SettingsGroup } from "../../ui/SettingsGroup";
import { AutostartToggle } from "../AutostartToggle";
import { StartHidden } from "../StartHidden";
import { ShowOverlay } from "../ShowOverlay";
import { ShowTrayIcon } from "../ShowTrayIcon";
import { HistoryLimit } from "../HistoryLimit";
import { RecordingRetentionPeriodSelector } from "../RecordingRetentionPeriod";
import { ClipboardHandlingSetting } from "../ClipboardHandling";
import { PasteMethodSetting } from "../PasteMethod";
import { TypingToolSetting } from "../TypingTool";
import { ModelUnloadTimeoutSetting } from "../ModelUnloadTimeout";
import { AppDataDirectory } from "../AppDataDirectory";
import { PasteDelay } from "../debug/PasteDelay";
import { LogLevelSelector } from "../debug/LogLevelSelector";
import { RecordingBuffer } from "../debug/RecordingBuffer";
import { SessionLimits } from "../SessionLimits";
import { AssistantProvider } from "../AssistantProvider";

const TABS = ["general", "system", "privacy", "advanced"] as const;
type SettingsTab = (typeof TABS)[number];

export const SettingsHub: React.FC = () => {
  const { t } = useTranslation();
  const [tab, setTab] = useState<SettingsTab>("general");

  // F012: deep-linkable tab — the assistant panel's "Abrir configurações"
  // emits `hub://navigate` with `settingsTab` to land directly on the
  // tab carrying the assistant controls (App.tsx handles `section`).
  useEffect(() => {
    const unlisten = listen<{ settingsTab?: string }>(
      "hub://navigate",
      (event) => {
        const settingsTab = event.payload.settingsTab;
        if (settingsTab && (TABS as readonly string[]).includes(settingsTab)) {
          setTab(settingsTab as SettingsTab);
        }
      },
    );
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  return (
    <main className="settings-hub">
      <header className="section-page-header">
        <h1>{t("settingsHub.title")}</h1>
        <p>{t("settingsHub.subtitle")}</p>
      </header>
      <nav
        className="settings-tabs"
        aria-label={t("settingsHub.sectionsLabel")}
      >
        {TABS.map((value) => (
          <button
            key={value}
            type="button"
            aria-current={tab === value ? "page" : undefined}
            onClick={() => setTab(value)}
          >
            {t(`settingsHub.tabs.${value}`)}
          </button>
        ))}
      </nav>
      <section className="settings-panel">
        {tab === "general" ? <GeneralSettings /> : null}
        {tab === "system" ? <SystemSettings /> : null}
        {tab === "privacy" ? <PrivacySettings /> : null}
        {tab === "advanced" ? <AdvancedHubSettings /> : null}
      </section>
    </main>
  );
};

const SystemSettings: React.FC = () => {
  const { t } = useTranslation();
  return (
    <div className="max-w-3xl w-full space-y-6">
      <SettingsGroup title={t("settingsHub.groups.startup")}>
        <AutostartToggle descriptionMode="tooltip" grouped />
        <StartHidden descriptionMode="tooltip" grouped />
        <ShowTrayIcon descriptionMode="tooltip" grouped />
      </SettingsGroup>
      <SettingsGroup title={t("settingsHub.groups.flowbar")}>
        <ShowOverlay descriptionMode="tooltip" grouped />
      </SettingsGroup>
    </div>
  );
};

const PrivacySettings: React.FC = () => {
  const { t } = useTranslation();
  return (
    <div className="max-w-3xl w-full space-y-6">
      <SettingsGroup title={t("settingsHub.groups.retention")}>
        <HistoryLimit descriptionMode="tooltip" grouped />
        <RecordingRetentionPeriodSelector descriptionMode="tooltip" grouped />
      </SettingsGroup>
      <SettingsGroup title={t("settingsHub.groups.clipboard")}>
        <ClipboardHandlingSetting descriptionMode="tooltip" grouped />
      </SettingsGroup>
    </div>
  );
};

const AdvancedHubSettings: React.FC = () => {
  const { t } = useTranslation();
  return (
    <div className="max-w-3xl w-full space-y-6">
      <SettingsGroup title={t("settingsHub.groups.insertion")}>
        <PasteMethodSetting descriptionMode="tooltip" grouped />
        <TypingToolSetting descriptionMode="tooltip" grouped />
        <PasteDelay descriptionMode="tooltip" grouped />
        <PasteDelay
          descriptionMode="tooltip"
          grouped
          settingKey="paste_delay_after_ms"
          labelKey="settings.debug.pasteDelayAfter.title"
          descriptionKey="settings.debug.pasteDelayAfter.description"
        />
      </SettingsGroup>
      {/* F012/T-091: the assistant overlay is a v1 feature — its provider
          stays visible regardless of `post_process_enabled` (the
          post-processing screen itself is still experimental). */}
      <SettingsGroup title={t("settingsHub.groups.assistant")}>
        <AssistantProvider descriptionMode="tooltip" grouped />
      </SettingsGroup>
      <SettingsGroup title={t("settingsHub.groups.performance")}>
        <ModelUnloadTimeoutSetting descriptionMode="tooltip" grouped />
        <RecordingBuffer descriptionMode="tooltip" grouped />
        <SessionLimits grouped />
      </SettingsGroup>
      <SettingsGroup title={t("settingsHub.groups.diagnostics")}>
        <LogLevelSelector descriptionMode="tooltip" grouped />
        <AppDataDirectory descriptionMode="tooltip" grouped />
      </SettingsGroup>
    </div>
  );
};
