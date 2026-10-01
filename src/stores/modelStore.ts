import { create } from "zustand";
import { subscribeWithSelector } from "zustand/middleware";
import { produce } from "immer";
import { listen } from "@tauri-apps/api/event";
import {
  commands,
  type ImportedModel,
  type ModelInfo,
  type ModelRecommendations,
} from "@/bindings";
import { toast } from "sonner";
import { useSettingsStore } from "./settingsStore";
import { localModelProviderId, type SttUsage } from "@/lib/providers";

interface DownloadProgress {
  model_id: string;
  downloaded: number;
  total: number;
  percentage: number;
}

interface DownloadStats {
  startTime: number;
  lastUpdate: number;
  totalDownloaded: number;
  speed: number; // MB/s
}

// Using Record instead of Set/Map for Immer compatibility
interface ModelsStore {
  models: ModelInfo[];
  currentModel: string;
  downloadingModels: Record<string, true>;
  verifyingModels: Record<string, true>;
  extractingModels: Record<string, true>;
  downloadProgress: Record<string, DownloadProgress>;
  downloadStats: Record<string, DownloadStats>;
  loading: boolean;
  error: string | null;
  initialized: boolean;
  isRescanning: boolean;
  /** Hardware probe + per-model suitability labels (FR-003-05). */
  recommendations: ModelRecommendations | null;

  // Actions
  initialize: () => Promise<void>;
  loadModels: () => Promise<void>;
  loadCurrentModel: () => Promise<void>;
  rescanLocalModels: () => Promise<void>;
  selectModel: (modelId: string) => Promise<boolean>;
  downloadModel: (modelId: string) => Promise<boolean>;
  cancelDownload: (modelId: string) => Promise<boolean>;
  deleteModel: (modelId: string) => Promise<boolean>;
  /** Import a local model file picked by the user (FR-003-07). */
  importModel: (path: string) => Promise<ImportedModel | null>;
  /** Hardware suitability labels; fetched lazily by the models screen. */
  loadRecommendations: () => Promise<void>;
  /**
   * Select the STT provider for a usage slot (FR-003-03). `modelId` is turned
   * into the `local_model:` provider id; `null` clears meeting/fallback.
   * Dictation delegates to `selectModel` (it is the real engine switch).
   */
  setProviderForUsage: (
    usage: SttUsage,
    modelId: string | null,
  ) => Promise<boolean>;
  getModelInfo: (modelId: string) => ModelInfo | undefined;
  isModelDownloading: (modelId: string) => boolean;
  isModelVerifying: (modelId: string) => boolean;
  isModelExtracting: (modelId: string) => boolean;
  getDownloadProgress: (modelId: string) => DownloadProgress | undefined;

  // Internal setters
  setModels: (models: ModelInfo[]) => void;
  setCurrentModel: (modelId: string) => void;
  setError: (error: string | null) => void;
  setLoading: (loading: boolean) => void;
}

export const useModelStore = create<ModelsStore>()(
  subscribeWithSelector((set, get) => ({
    models: [],
    currentModel: "",
    downloadingModels: {},
    verifyingModels: {},
    extractingModels: {},
    downloadProgress: {},
    downloadStats: {},
    loading: true,
    error: null,
    initialized: false,
    isRescanning: false,
    recommendations: null,

    // Internal setters
    setModels: (models) => set({ models }),
    setCurrentModel: (currentModel) => set({ currentModel }),
    setError: (error) => set({ error }),
    setLoading: (loading) => set({ loading }),

    loadModels: async () => {
      try {
        const result = await commands.getAvailableModels();
        if (result.status === "ok") {
          set({ models: result.data, error: null });

          // Sync downloading state from backend
          set(
            produce((state) => {
              const backendDownloading: Record<string, true> = {};
              result.data
                .filter((m) => m.is_downloading)
                .forEach((m) => {
                  backendDownloading[m.id] = true;
                });

              // Merge: keep frontend state if downloading, add backend state
              Object.keys(backendDownloading).forEach((id) => {
                state.downloadingModels[id] = true;
              });

              // Remove models that backend says are NOT downloading AND
              // frontend doesn't have progress for (completed/cancelled)
              Object.keys(state.downloadingModels).forEach((id) => {
                if (!backendDownloading[id] && !state.downloadProgress[id]) {
                  delete state.downloadingModels[id];
                }
              });
            }),
          );
        } else {
          set({ error: `Failed to load models: ${result.error.message}` });
        }
      } catch (err) {
        set({ error: `Failed to load models: ${err}` });
      } finally {
        set({ loading: false });
      }
    },

    loadCurrentModel: async () => {
      try {
        const result = await commands.getCurrentModel();
        if (result.status === "ok") {
          set({ currentModel: result.data });
        }
      } catch (err) {
        console.error("Failed to load current model:", err);
      }
    },

    rescanLocalModels: async () => {
      set({ isRescanning: true });
      try {
        const result = await commands.rescanLocalModels();
        if (result.status !== "ok") {
          set({ error: `Failed to rescan models: ${result.error.message}` });
        }
        // On success the backend emits `models-updated`, which reloads the list
        // via the listener registered in initialize().
      } catch (err) {
        set({ error: `Failed to rescan models: ${err}` });
      } finally {
        set({ isRescanning: false });
      }
    },

    selectModel: async (modelId: string) => {
      try {
        set({ error: null });
        const result = await commands.setActiveModel(modelId);
        if (result.status === "ok") {
          set({ currentModel: modelId });
          return true;
        } else {
          set({ error: `Failed to switch to model: ${result.error.message}` });
          return false;
        }
      } catch (err) {
        set({ error: `Failed to switch to model: ${err}` });
        return false;
      }
    },

    downloadModel: async (modelId: string) => {
      try {
        set({ error: null });
        set(
          produce((state) => {
            state.downloadingModels[modelId] = true;
            state.downloadProgress[modelId] = {
              model_id: modelId,
              downloaded: 0,
              total: 0,
              percentage: 0,
            };
          }),
        );
        const result = await commands.downloadModel(modelId);
        if (result.status !== "ok") {
          // Fallback cleanup in case the model-download-failed event was not received
          // (e.g. listener not yet registered). The event handler is a no-op if it
          // arrives after this cleanup since deleting missing keys is safe.
          set(
            produce((state) => {
              delete state.downloadingModels[modelId];
              delete state.downloadProgress[modelId];
              delete state.downloadStats[modelId];
            }),
          );
        }
        return result.status === "ok";
      } catch {
        // model-download-failed event won't fire for JS exceptions (e.g. IPC error),
        // so clean up state here to avoid a stuck progress spinner.
        set(
          produce((state) => {
            delete state.downloadingModels[modelId];
            delete state.downloadProgress[modelId];
            delete state.downloadStats[modelId];
          }),
        );
        return false;
      }
    },

    cancelDownload: async (modelId: string) => {
      try {
        set({ error: null });
        const result = await commands.cancelDownload(modelId);
        if (result.status === "ok") {
          set(
            produce((state) => {
              delete state.downloadingModels[modelId];
              delete state.downloadProgress[modelId];
              delete state.downloadStats[modelId];
            }),
          );

          // Reload models to sync with backend state
          await get().loadModels();
          return true;
        } else {
          set({ error: `Failed to cancel download: ${result.error.message}` });
          return false;
        }
      } catch (err) {
        set({ error: `Failed to cancel download: ${err}` });
        return false;
      }
    },

    deleteModel: async (modelId: string) => {
      try {
        set({ error: null });
        const result = await commands.deleteModel(modelId);
        if (result.status === "ok") {
          await get().loadModels();
          await get().loadCurrentModel();
          // Deleting a model can also clear provider selections that pointed
          // at it (meeting/fallback) — resync settings so usage labels drop.
          await useSettingsStore.getState().refreshSettings();
          return true;
        } else {
          set({ error: `Failed to delete model: ${result.error.message}` });
          return false;
        }
      } catch (err) {
        set({ error: `Failed to delete model: ${err}` });
        return false;
      }
    },

    importModel: async (path: string) => {
      try {
        set({ error: null });
        const result = await commands.importModel(path);
        if (result.status === "ok") {
          // The import's rescan emits `models-updated` which reloads the list,
          // but reload defensively in case the listener is not attached yet.
          await get().loadModels();
          return result.data;
        }
        set({ error: result.error.message });
        return null;
      } catch (err) {
        set({ error: `Failed to import model: ${err}` });
        return null;
      }
    },

    loadRecommendations: async () => {
      if (get().recommendations) return;
      try {
        const result = await commands.getModelRecommendations();
        if (result.status === "ok") {
          set({ recommendations: result.data });
        }
      } catch (err) {
        // The probe only informs labels — never block the screen on it.
        console.warn("Failed to load model recommendations:", err);
      }
    },

    setProviderForUsage: async (usage: SttUsage, modelId: string | null) => {
      // Dictation selection is the engine switch itself (selected_model); the
      // backend resolves a null dictation provider to it.
      if (usage === "dictation") {
        if (modelId === null) {
          set({ error: "Dictation requires a model" });
          return false;
        }
        return get().selectModel(modelId);
      }
      try {
        set({ error: null });
        const result = await commands.setSttProvider(
          usage,
          modelId === null ? null : localModelProviderId(modelId),
        );
        if (result.status === "ok") {
          await useSettingsStore.getState().refreshSettings();
          return true;
        }
        set({ error: result.error.message });
        return false;
      } catch (err) {
        set({ error: `Failed to set provider: ${err}` });
        return false;
      }
    },

    getModelInfo: (modelId: string) => {
      return get().models.find((model) => model.id === modelId);
    },

    isModelDownloading: (modelId: string) => {
      return modelId in get().downloadingModels;
    },

    isModelVerifying: (modelId: string) => {
      return modelId in get().verifyingModels;
    },

    isModelExtracting: (modelId: string) => {
      return modelId in get().extractingModels;
    },

    getDownloadProgress: (modelId: string) => {
      return get().downloadProgress[modelId];
    },

    initialize: async () => {
      if (get().initialized) return;

      const { loadModels, loadCurrentModel } = get();

      // Load initial data
      await Promise.all([loadModels(), loadCurrentModel()]);

      // Set up event listeners
      listen<DownloadProgress>("model-download-progress", (event) => {
        const progress = event.payload;
        set(
          produce((state) => {
            state.downloadProgress[progress.model_id] = progress;
          }),
        );

        // Update download stats for speed calculation
        const now = Date.now();
        set(
          produce((state) => {
            const current = state.downloadStats[progress.model_id];

            if (!current) {
              state.downloadStats[progress.model_id] = {
                startTime: now,
                lastUpdate: now,
                totalDownloaded: progress.downloaded,
                speed: 0,
              };
            } else {
              const timeDiff = (now - current.lastUpdate) / 1000;
              const bytesDiff = progress.downloaded - current.totalDownloaded;

              if (timeDiff > 0.5) {
                const currentSpeed = bytesDiff / (1024 * 1024) / timeDiff;
                const validCurrentSpeed = Math.max(0, currentSpeed);
                const smoothedSpeed =
                  current.speed > 0
                    ? current.speed * 0.8 + validCurrentSpeed * 0.2
                    : validCurrentSpeed;

                state.downloadStats[progress.model_id] = {
                  startTime: current.startTime,
                  lastUpdate: now,
                  totalDownloaded: progress.downloaded,
                  speed: Math.max(0, smoothedSpeed),
                };
              }
            }
          }),
        );
      });

      listen<string>("model-download-complete", (event) => {
        const modelId = event.payload;
        set(
          produce((state) => {
            delete state.downloadingModels[modelId];
            delete state.verifyingModels[modelId];
            delete state.downloadProgress[modelId];
            delete state.downloadStats[modelId];
          }),
        );
        get().loadModels();
      });

      listen<{ model_id: string; error: string }>(
        "model-download-failed",
        (event) => {
          const { model_id: modelId, error } = event.payload;
          set(
            produce((state) => {
              delete state.downloadingModels[modelId];
              delete state.verifyingModels[modelId];
              delete state.downloadProgress[modelId];
              delete state.downloadStats[modelId];
              state.error = error;
            }),
          );
          toast.error(error);
        },
      );

      listen<string>("model-verification-started", (event) => {
        const modelId = event.payload;
        set(
          produce((state) => {
            state.verifyingModels[modelId] = true;
          }),
        );
      });

      listen<string>("model-verification-completed", (event) => {
        const modelId = event.payload;
        set(
          produce((state) => {
            delete state.verifyingModels[modelId];
          }),
        );
      });

      listen<string>("model-extraction-started", (event) => {
        const modelId = event.payload;
        set(
          produce((state) => {
            state.extractingModels[modelId] = true;
          }),
        );
      });

      listen<string>("model-extraction-completed", (event) => {
        const modelId = event.payload;
        set(
          produce((state) => {
            delete state.extractingModels[modelId];
          }),
        );
        get().loadModels();
      });

      listen<{ model_id: string; error: string }>(
        "model-extraction-failed",
        (event) => {
          const modelId = event.payload.model_id;
          set(
            produce((state) => {
              delete state.extractingModels[modelId];
              state.error = `Failed to extract model: ${event.payload.error}`;
            }),
          );
        },
      );

      listen<string>("model-download-cancelled", (event) => {
        const modelId = event.payload;
        set(
          produce((state) => {
            delete state.downloadingModels[modelId];
            delete state.verifyingModels[modelId];
            delete state.downloadProgress[modelId];
            delete state.downloadStats[modelId];
          }),
        );
      });

      listen<string>("model-deleted", () => {
        get().loadModels();
        get().loadCurrentModel();
      });

      listen("model-state-changed", () => {
        get().loadModels();
        get().loadCurrentModel();
      });

      listen("models-updated", () => {
        get().loadModels();
        get().loadCurrentModel();
      });

      set({ initialized: true });
    },
  })),
);
