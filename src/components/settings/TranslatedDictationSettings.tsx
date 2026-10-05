import React, { useState } from "react";
import {
  CircleCheck,
  Download,
  Info,
  LoaderCircle,
  TriangleAlert,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { useSettings } from "@/hooks/useSettings";
import { useModelStore } from "@/stores/modelStore";
import { SettingsGroup } from "../ui/SettingsGroup";
import { ShortcutInput } from "./ShortcutInput";
import {
  canTranslateToEnglish,
  getTranslationModels,
  translationModelState,
} from "./translationModels";

/** Icon and tone per state, so a model that can't translate reads as a warning. */
const STATE_PRESENTATION = {
  unselected: { Icon: Info, tone: "neutral" },
  incompatible: { Icon: TriangleAlert, tone: "warning" },
  downloading: { Icon: LoaderCircle, tone: "neutral" },
  downloadRequired: { Icon: Download, tone: "neutral" },
  ready: { Icon: CircleCheck, tone: "success" },
} as const;

/** Prepares translation; the global shortcut starts it in the destination app. */
export const TranslatedDictationSettings: React.FC = () => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating } = useSettings();
  const {
    models,
    currentModel,
    downloadingModels,
    verifyingModels,
    extractingModels,
    downloadModel,
    cancelDownload,
  } = useModelStore();
  const selected = getSetting("translation_model_id") ?? null;
  const effective = models.find(
    (model) => model.id === (selected ?? currentModel),
  );
  const busy =
    !!effective &&
    !!(
      downloadingModels[effective.id] ||
      verifyingModels[effective.id] ||
      extractingModels[effective.id]
    );
  const state = translationModelState(effective, busy);
  const options = getTranslationModels(models);
  const currentInfo = models.find((model) => model.id === currentModel);
  const { Icon: StateIcon, tone } = STATE_PRESENTATION[state];
  const [downloadError, setDownloadError] = useState(false);
  const runDownloadAction = async (cancel: boolean) => {
    if (!effective) return;
    setDownloadError(false);
    const success = await (cancel
      ? cancelDownload(effective.id)
      : downloadModel(effective.id));
    setDownloadError(!success);
  };

  return (
    <section data-testid="translated-dictation-settings">
      <SettingsGroup
        title={t("settings.translatedDictation.title")}
        description={t("settings.translatedDictation.description")}
      >
        <div className="p-4 space-y-3">
          <label className="block space-y-2">
            <span className="text-sm font-medium">
              {t("settings.translatedDictation.model")}
            </span>
            <select
              className="w-full rounded-md border border-mid-gray/30 bg-background p-2 text-sm"
              value={selected ?? ""}
              disabled={isUpdating("translation_model_id")}
              onChange={(event) => {
                setDownloadError(false);
                void updateSetting(
                  "translation_model_id",
                  event.target.value || null,
                );
              }}
              aria-describedby="translation-model-status"
            >
              <option value="">
                {!currentInfo || canTranslateToEnglish(currentInfo)
                  ? t("settings.translatedDictation.currentModel")
                  : t(
                      "settings.translatedDictation.currentModelCannotTranslate",
                      {
                        model: currentInfo?.name ?? "",
                      },
                    )}
              </option>
              {selected && !options.some((model) => model.id === selected) && (
                <option value={selected} disabled>
                  {t("settings.translatedDictation.unavailableModel")}
                </option>
              )}
              {options.map((model) => (
                <option key={model.id} value={model.id}>
                  {t(
                    model.is_downloaded
                      ? "settings.translatedDictation.optionDownloaded"
                      : "settings.translatedDictation.optionNotDownloaded",
                    { name: model.name },
                  )}
                </option>
              ))}
            </select>
          </label>
          <p className="text-sm text-mid-gray">
            {t("settings.translatedDictation.listHint")}
          </p>
          <p
            id="translation-model-status"
            role="status"
            className="st-note"
            data-tone={tone}
          >
            <StateIcon width={16} height={16} aria-hidden="true" />
            <span>
              {t(`settings.translatedDictation.states.${state}`, {
                model: effective?.name ?? "",
              })}
            </span>
          </p>
          {downloadError && (
            <p role="alert" className="text-sm">
              {t("settings.translatedDictation.downloadFailed")}
            </p>
          )}
          {state === "downloadRequired" && effective && (
            <button
              type="button"
              className="st-btn"
              onClick={() => void runDownloadAction(false)}
            >
              {t("settings.translatedDictation.download")}
            </button>
          )}
          {state === "downloading" &&
            effective &&
            downloadingModels[effective.id] && (
              <button
                type="button"
                className="st-btn"
                onClick={() => void runDownloadAction(true)}
              >
                {t("settings.translatedDictation.cancelDownload")}
              </button>
            )}
        </div>
        <ShortcutInput shortcutId="transcribe_translate" grouped />
      </SettingsGroup>
    </section>
  );
};
