import { useCallback, useEffect, useRef, useState } from "react";
import type { ModelInfo } from "@/bindings";
import { useModelStore } from "@/stores/modelStore";
import { pickSetupModel } from "../setupFlow";

export type SetupModelPhase =
  | "loading"
  | "downloading"
  | "verifying"
  | "extracting"
  | "activating"
  | "ready"
  | "error";

export interface SetupModelState {
  phase: SetupModelPhase;
  model: ModelInfo | undefined;
  progress: number | undefined;
  /** MB/s */
  speed: number | undefined;
  downloadedMb: number | undefined;
  retry: () => void;
}

/**
 * Downloads and activates the setup model (Parakeet V3) on its own: no list,
 * no choice. Uses the regular model store, so verification, extraction and
 * failure toasts behave exactly like a manual download.
 */
export const useSetupModel = (): SetupModelState => {
  const {
    models,
    initialized,
    downloadModel,
    selectModel,
    downloadingModels,
    verifyingModels,
    extractingModels,
    downloadProgress,
    downloadStats,
    currentModel,
  } = useModelStore();
  const [failed, setFailed] = useState(false);
  const [active, setActive] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const started = useRef(false);
  const activating = useRef(false);

  // The store is initialized once at boot (main.tsx).
  const model = pickSetupModel(models);
  const id = model?.id;

  // Kick off the download once, as soon as the catalog is known.
  useEffect(() => {
    if (!id || model?.is_downloaded || started.current) return;
    if (id in downloadingModels) {
      started.current = true;
      return;
    }
    started.current = true;
    void downloadModel(id).then((ok) => {
      if (!ok) setFailed(true);
    });
  }, [id, model?.is_downloaded, downloadingModels, downloadModel, attempt]);

  const busy =
    !!id &&
    (id in downloadingModels ||
      id in verifyingModels ||
      id in extractingModels);

  // Activate it once it is fully on disk.
  useEffect(() => {
    if (!id || !model?.is_downloaded || busy || active || activating.current) {
      return;
    }
    if (currentModel === id) {
      setActive(true);
      return;
    }
    activating.current = true;
    void selectModel(id).then((ok) => {
      activating.current = false;
      if (ok) setActive(true);
      else setFailed(true);
    });
  }, [
    id,
    model?.is_downloaded,
    busy,
    active,
    currentModel,
    selectModel,
    attempt,
  ]);

  const retry = useCallback(() => {
    started.current = false;
    activating.current = false;
    setFailed(false);
    setAttempt((n) => n + 1);
  }, []);

  const progress = id ? downloadProgress[id] : undefined;
  let phase: SetupModelPhase;
  if (failed) phase = "error";
  else if (!initialized || !model) phase = initialized ? "error" : "loading";
  else if (active) phase = "ready";
  else if (id! in extractingModels) phase = "extracting";
  else if (id! in verifyingModels) phase = "verifying";
  else if (model.is_downloaded) phase = "activating";
  else phase = "downloading";

  return {
    phase,
    model,
    progress: progress?.percentage,
    speed: id ? downloadStats[id]?.speed : undefined,
    downloadedMb:
      progress?.downloaded !== undefined
        ? progress.downloaded / (1024 * 1024)
        : undefined,
    retry,
  };
};
