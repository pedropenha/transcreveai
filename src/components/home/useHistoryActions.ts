import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { commands, type HistoryEntry, type ModelInfo } from "@/bindings";
import { useSettings } from "@/hooks/useSettings";
import { copyToClipboard } from "../settings/history/clipboard";

/** Row/detail actions for a history entry (FR-010-04), with user feedback. */
export function useHistoryActions(onDeleted: (id: number) => void) {
  const { t } = useTranslation();
  const { getSetting, updateSetting } = useSettings();
  const [retrying, setRetrying] = useState<number | null>(null);
  const [retryModels, setRetryModels] = useState<ModelInfo[]>([]);
  const [retryModelId, setRetryModelId] = useState("");

  useEffect(() => {
    void Promise.all([
      commands.getAvailableModels(),
      commands.getEffectiveSttModels(),
    ]).then(([modelsResult, effectiveResult]) => {
      if (modelsResult.status === "ok") {
        setRetryModels(
          modelsResult.data.filter((model) => model.is_downloaded),
        );
      }
      if (effectiveResult.status === "ok") {
        setRetryModelId(effectiveResult.data.dictation ?? "");
      }
    });
  }, []);

  const copy = useCallback(
    async (entry: HistoryEntry) => {
      const copied = await copyToClipboard(entry.final_text);
      if (copied) toast.success(t("settings.history.copied"));
      else toast.error(t("settings.history.copyError"));
    },
    [t],
  );

  const reinsert = useCallback(
    async (entry: HistoryEntry) => {
      const result = await commands.reinsertHistoryEntry(entry.id);
      if (result.status === "ok") {
        toast.success(t("settings.history.reinserted"));
      } else {
        toast.error(t("settings.history.reinsertError"));
      }
    },
    [t],
  );

  const toggleFlag = useCallback(async (entry: HistoryEntry) => {
    await commands.toggleHistoryEntrySaved(entry.id);
  }, []);

  const remove = useCallback(
    async (entry: HistoryEntry) => {
      if (!window.confirm(t("settings.history.deleteConfirm"))) return;
      const result = await commands.deleteHistoryEntry(entry.id);
      if (result.status !== "ok") {
        toast.error(t("settings.history.deleteError"));
        return;
      }
      onDeleted(entry.id);
      toast.success(t("settings.history.deleted"));
    },
    [onDeleted, t],
  );

  const retry = useCallback(
    async (entry: HistoryEntry) => {
      setRetrying(entry.id);
      try {
        if (retryModelId) {
          const providerResult = await commands.setSttProvider(
            "dictation",
            `local_model:${retryModelId}`,
          );
          if (providerResult.status !== "ok") {
            toast.error(t("settings.history.retranscribeError"));
            return;
          }
        }
        const result = await commands.retryHistoryEntryTranscription(entry.id);
        if (result.status !== "ok") {
          toast.error(t("settings.history.retranscribeError"));
        }
      } catch {
        toast.error(t("settings.history.retranscribeError"));
      } finally {
        setRetrying(null);
      }
    },
    [retryModelId, t],
  );

  const addToDictionary = useCallback(
    async (input = "") => {
      const term = (input || window.getSelection()?.toString() || "")
        .replace(/\s+/g, " ")
        .trim();
      if (!term) {
        toast.error(t("settings.history.selectDictionaryText"));
        return;
      }
      const words = getSetting("custom_words") ?? [];
      if (!words.includes(term)) {
        await updateSetting("custom_words", [...words, term]);
      }
      toast.success(t("settings.history.dictionaryAdded", { term }));
    },
    [getSetting, t, updateSetting],
  );

  return {
    retrying,
    retryModels,
    retryModelId,
    setRetryModelId,
    copy,
    reinsert,
    toggleFlag,
    remove,
    retry,
    addToDictionary,
  };
}
