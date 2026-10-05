import React from "react";
import { useTranslation } from "react-i18next";
import { type } from "@tauri-apps/plugin-os";
import { Info, KeyRound, Lock } from "lucide-react";
import { useSettings } from "@/hooks/useSettings";
import { useModelStore } from "@/stores/modelStore";
import { getLanguageLabel } from "@/lib/constants/languages";
import { SettingsGroup } from "../../ui/SettingsGroup";
import { ModelsSettings } from "../models/ModelsSettings";
import { ModelSettingsCard } from "../general/ModelSettingsCard";
import { AppLanguageSelector } from "../AppLanguageSelector";
import { ThemeSelector } from "../ThemeSelector";
import { LanguageSelector } from "../LanguageSelector";
import { ShortcutInput } from "../ShortcutInput";
import { ShortcutActivationSetting } from "../ShortcutActivation";
import { MicrophoneSelector } from "../MicrophoneSelector";
import { ChannelSelector } from "../ChannelSelector";
import { OutputDeviceSelector } from "../OutputDeviceSelector";
import { AudioFeedback } from "../AudioFeedback";
import { VolumeSlider } from "../VolumeSlider";
import { MuteWhileRecording } from "../MuteWhileRecording";
import { AssistantProvider } from "../AssistantProvider";
import { PostProcessingSettingsApi } from "../PostProcessingSettingsApi";
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
import { TranslatedDictationSettings } from "../TranslatedDictationSettings";
import type { SettingsPage } from "./settingsNav";
import { ConnectorSettings } from "../connectors/ConnectorSettings";

const GeneralPage: React.FC = () => {
  const { t } = useTranslation();
  return (
    <div className="st-stack">
      <SettingsGroup title={t("settingsHub.groups.appearance")}>
        <ThemeSelector descriptionMode="tooltip" grouped />
        <AppLanguageSelector descriptionMode="tooltip" grouped />
      </SettingsGroup>
    </div>
  );
};

const ShortcutsPage: React.FC = () => {
  const { t } = useTranslation();
  const isLinux = type() === "linux";
  return (
    <div className="st-stack">
      <SettingsGroup title={t("settings.general.title")}>
        <ShortcutInput shortcutId="transcribe" grouped />
        <ShortcutInput shortcutId="transcribe_translate" grouped />
        <ShortcutActivationSetting descriptionMode="tooltip" grouped />
        {/* Cancel shortcut stays hidden on Linux (dynamic shortcut instability). */}
        {!isLinux && <ShortcutInput shortcutId="cancel" grouped />}
        {/* F012/T-091: assistant overlay hotkey (FR-012-10). */}
        <ShortcutInput shortcutId="assistant" grouped />
      </SettingsGroup>
    </div>
  );
};

const AudioPage: React.FC = () => {
  const { t } = useTranslation();
  const { audioFeedbackEnabled } = useSettings();
  return (
    <div className="st-stack">
      <SettingsGroup title={t("settings.sound.title")}>
        <MicrophoneSelector descriptionMode="tooltip" grouped />
        <ChannelSelector descriptionMode="tooltip" grouped />
        <MuteWhileRecording descriptionMode="tooltip" grouped />
        <AudioFeedback descriptionMode="tooltip" grouped />
        <OutputDeviceSelector
          descriptionMode="tooltip"
          grouped
          disabled={!audioFeedbackEnabled}
        />
        <VolumeSlider disabled={!audioFeedbackEnabled} />
      </SettingsGroup>
    </div>
  );
};

const LanguagesPage: React.FC = () => {
  const { t } = useTranslation();
  const { currentModel, models } = useModelStore();
  const active = models.find((m) => m.id === currentModel);
  const codes = (active?.supported_languages ?? []).filter(
    (code) => code !== "auto",
  );
  return (
    <div className="st-stack">
      <SettingsGroup title={t("settings.general.language.title")}>
        <LanguageSelector descriptionMode="tooltip" grouped />
      </SettingsGroup>
      <ModelSettingsCard />
      <TranslatedDictationSettings />
      {active && codes.length > 0 ? (
        <section className="st-section" aria-labelledby="active-langs">
          <h2 id="active-langs" className="caps">
            {t("settingsHub.languages.supportedBy", { model: active.name })}
          </h2>
          <ul className="st-lang-list">
            {codes.map((code) => (
              <li key={code} className="st-chip st-chip-sm" data-tone="outline">
                {getLanguageLabel(code) || code}
              </li>
            ))}
          </ul>
        </section>
      ) : null}
    </div>
  );
};

/** Product names shown in the disabled preview (not translatable). */
const API_PREVIEW = { provider: "OpenAI", model: "gpt-4o-transcribe" } as const;

/** Transcription through an API key (F003): v1.1+, shown disabled. */
const ApiPage: React.FC = () => {
  const { t } = useTranslation();
  return (
    <div className="st-stack">
      <div className="st-api-card" data-testid="api-card">
        <div className="st-api-head">
          <span className="st-api-icon" aria-hidden="true">
            <KeyRound width={18} height={18} />
          </span>
          <div>
            <b>{t("settingsHub.api.cardTitle")}</b>
            <p>{t("settingsHub.api.cardBody")}</p>
          </div>
          <span className="st-chip st-chip-sm" data-tone="outline">
            {t("settingsHub.soon")}
          </span>
        </div>
        <div className="st-api-grid">
          <label>
            <span>{t("settingsHub.api.provider")}</span>
            <select disabled>
              <option>{API_PREVIEW.provider}</option>
            </select>
          </label>
          <label>
            <span>{t("settingsHub.api.model")}</span>
            <select disabled>
              <option>{API_PREVIEW.model}</option>
            </select>
          </label>
        </div>
        <label>
          <span>{t("settingsHub.api.key")}</span>
          <input type="password" name="api-key" autoComplete="off" disabled />
        </label>
        <p className="st-vault">
          <Lock width={13} height={13} aria-hidden="true" />
          {t("settingsHub.api.vault")}
        </p>
      </div>
      <p className="st-note">
        <Info width={13} height={13} aria-hidden="true" />
        {t("settingsHub.api.summaryNote")}
      </p>
    </div>
  );
};

const SummariesPage: React.FC = () => {
  const { t } = useTranslation();
  return (
    <div className="st-stack">
      {/* FR-009: meeting-summary provider — the same
          `post_process_provider_id` the summary pipeline uses. */}
      <SettingsGroup title={t("settingsHub.groups.summary")}>
        <PostProcessingSettingsApi />
      </SettingsGroup>
    </div>
  );
};

const AssistantPage: React.FC = () => {
  const { t } = useTranslation();
  return (
    <div className="st-stack">
      {/* F012: Codex/Claude are detected on PATH, no key needed (FR-012-02/04). */}
      <SettingsGroup title={t("settingsHub.groups.assistant")}>
        <AssistantProvider descriptionMode="tooltip" grouped />
      </SettingsGroup>
      <p className="st-note">
        <Info width={13} height={13} aria-hidden="true" />
        {t("settingsHub.assistant.shortcutNote")}
      </p>
    </div>
  );
};

const SystemPage: React.FC = () => {
  const { t } = useTranslation();
  return (
    <div className="st-stack">
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

const PrivacyPage: React.FC = () => {
  const { t } = useTranslation();
  return (
    <div className="st-stack">
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

const AdvancedPage: React.FC = () => {
  const { t } = useTranslation();
  return (
    <div className="st-stack">
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

const PAGES: Record<SettingsPage, React.FC> = {
  "usage/general": GeneralPage,
  "usage/shortcuts": ShortcutsPage,
  "usage/audio": AudioPage,
  "transcription/models": ModelsSettings,
  "transcription/languages": LanguagesPage,
  "transcription/api": ApiPage,
  "intelligence/summaries": SummariesPage,
  "intelligence/assistant": AssistantPage,
  "intelligence/connectors": ConnectorSettings,
  "app/system": SystemPage,
  "app/privacy": PrivacyPage,
  "app/advanced": AdvancedPage,
};

export const SettingsPageContent: React.FC<{ page: SettingsPage }> = ({
  page,
}) => {
  const Page = PAGES[page];
  return <Page />;
};
