import { test, expect } from "@playwright/test";
import { installTauriMock } from "./helpers/tauri-mock";

/**
 * "Models & Providers" screen (T-043): catalog listing, usage selectors,
 * import/delete/select flows — all against mocked IPC.
 */

const catalogModel = (overrides: Record<string, unknown>) => ({
  id: "whisper-small",
  name: "Whisper Small",
  description: "Balanced model",
  filename: "ggml-small.bin",
  source: { Url: { url: "https://example.invalid/m.bin", sha256: "aa" } },
  size_mb: 466,
  is_downloaded: true,
  is_downloading: false,
  partial_size: 0,
  is_directory: false,
  engine_type: "TranscribeCpp",
  accuracy_score: 0.7,
  speed_score: 0.8,
  supports_translation: true,
  is_recommended: false,
  supported_languages: ["auto"],
  supports_language_selection: true,
  is_custom: false,
  supports_streaming: false,
  supports_language_detection: true,
  ...overrides,
});

const SETTINGS = {
  onboarding_completed: true,
  app_language: "en",
  debug_mode: false,
  post_process_enabled: false,
  selected_model: "whisper-small",
  dictation_provider_id: null,
  meeting_provider_id: null,
  fallback_provider_id: null,
};

const MODELS = [
  catalogModel({}),
  // HuggingFace-sourced catalog entry: Url-sourced (legacy) downloads that are
  // not on disk are hidden from the list, so this keeps Turbo visible.
  catalogModel({
    id: "whisper-turbo",
    name: "Whisper Turbo",
    is_downloaded: false,
    is_recommended: true,
    source: {
      HuggingFace: { repo_id: "ggerganov/whisper.cpp", revision: "main" },
    },
  }),
];

test.describe("Models & Providers screen (mocked Tauri IPC)", () => {
  test("lists catalog models, providers, and usage selectors", async ({
    page,
  }) => {
    await installTauriMock(page, {
      get_app_settings: SETTINGS,
      get_available_models: MODELS,
      get_current_model: "whisper-small",
      get_model_recommendations: {
        hardware: {},
        default_model_id: "whisper-turbo",
        labels: [{ model_id: "whisper-turbo", label: "recommended" }],
      },
    });
    await page.goto("/");

    await page.getByTitle("Settings").click();
    await page.getByRole("button", { name: "Models", exact: true }).click();

    // Providers block: local families only, no cloud entries.
    await expect(
      page.getByRole("heading", { name: "Providers", exact: true }),
    ).toBeVisible();
    await expect(page.getByText("Whisper", { exact: true })).toBeVisible();
    await expect(page.getByText("1 of 2 models downloaded")).toBeVisible();
    await expect(
      page.getByText(/Only local models are available in this version/),
    ).toBeVisible();

    // Usage selectors show the inherited meeting pick.
    await expect(page.getByRole("heading", { name: "Usage" })).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Same as dictation" }),
    ).toBeVisible();

    // Active model in the hero, the rest in the catalog table.
    await expect(
      page.getByTestId("model-hero").getByRole("heading", {
        name: /Whisper Small/,
      }),
    ).toBeVisible();
    await expect(
      page.getByRole("table", { name: "Models" }).getByText("Whisper Turbo"),
    ).toBeVisible();
  });

  test("badges exactly the models that translate to English", async ({
    page,
  }) => {
    const hf = {
      HuggingFace: { repo_id: "ggerganov/whisper.cpp", revision: "main" },
    };
    await installTauriMock(page, {
      get_app_settings: SETTINGS,
      get_available_models: [
        catalogModel({}),
        catalogModel({
          id: "whisper-medium",
          name: "Whisper Medium",
          is_downloaded: false,
          source: hf,
        }),
        catalogModel({
          id: "moonshine-tiny",
          name: "Moonshine Tiny",
          is_downloaded: false,
          supports_translation: false,
          source: hf,
        }),
      ],
      get_current_model: "whisper-small",
      get_model_recommendations: {
        hardware: {},
        default_model_id: "whisper-small",
        labels: [],
      },
    });
    await page.goto("/");
    await page.getByTitle("Settings").click();
    await page.getByRole("button", { name: "Models", exact: true }).click();

    const table = page.getByRole("table", { name: "Models" });
    await expect(
      table
        .getByRole("row", { name: /Whisper Medium/ })
        .getByText("Translates to English"),
    ).toBeVisible();
    await expect(
      table
        .getByRole("row", { name: /Moonshine Tiny/ })
        .getByText("Translates to English"),
    ).toHaveCount(0);
  });

  test("meeting pick sends a local_model provider id", async ({ page }) => {
    const mock = await installTauriMock(page, {
      get_app_settings: SETTINGS,
      get_available_models: MODELS,
      get_current_model: "whisper-small",
      get_model_recommendations: null,
      set_stt_provider: null,
    });
    await page.goto("/");
    await page.getByTitle("Settings").click();
    await page.getByRole("button", { name: "Models", exact: true }).click();
    await expect(
      page.getByRole("heading", { name: "Whisper Small" }),
    ).toBeVisible();

    // The meeting row starts on "Same as dictation"; open it and pick the
    // model option (index 1 — "Same as dictation" is index 0 and its
    // description also contains the dictation model's name).
    await page.getByRole("button", { name: "Same as dictation" }).click();
    await page.locator("div.absolute button").nth(1).click();

    await expect
      .poll(() =>
        mock.calls
          .filter((c) => c.cmd === "set_stt_provider")
          .map((c) => c.args),
      )
      .toContainEqual({
        usage: "meeting",
        providerId: "local_model:whisper-small",
      });
  });

  test("dictation pick switches the active model", async ({ page }) => {
    const mock = await installTauriMock(page, {
      get_app_settings: SETTINGS,
      get_available_models: MODELS,
      get_current_model: "whisper-small",
      get_model_recommendations: null,
      set_active_model: null,
    });
    await page.goto("/");
    await page.getByTitle("Settings").click();
    await page.getByRole("button", { name: "Models", exact: true }).click();
    await expect(
      page.getByRole("heading", { name: "Whisper Small" }),
    ).toBeVisible();

    // Dictation is the engine switch itself: it goes through set_active_model
    // (selected_model), not the provider-id settings.
    await page
      .getByRole("button", { name: "Whisper Small", exact: true })
      .first()
      .click();
    await page
      .locator("div.absolute button", { hasText: "Whisper Small" })
      .first()
      .click();

    await expect
      .poll(() =>
        mock.calls
          .filter((c) => c.cmd === "set_active_model")
          .map((c) => c.args),
      )
      .toContainEqual({ modelId: "whisper-small" });
  });
});
