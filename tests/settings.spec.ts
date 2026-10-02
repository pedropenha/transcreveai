import { test, expect, type Page } from "@playwright/test";
import { emitTauriEvent, installTauriMock } from "./helpers/tauri-mock";

const model = (overrides: Record<string, unknown>) => ({
  id: "whisper-small",
  name: "Whisper Small",
  description: "Light, for laptops",
  filename: "ggml-small.bin",
  source: {
    HuggingFace: { repo_id: "ggerganov/whisper.cpp", revision: "main" },
  },
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
  supported_languages: ["pt", "en", "es"],
  supports_language_selection: true,
  is_custom: false,
  supports_streaming: false,
  supports_language_detection: true,
  ...overrides,
});

const MODELS = [
  model({}),
  model({
    id: "whisper-turbo",
    name: "Whisper Turbo",
    description: "Best balance",
    size_mb: 1638,
    is_downloaded: false,
    is_recommended: true,
    accuracy_score: 1,
    speed_score: 0.8,
  }),
  model({
    id: "whisper-medium",
    name: "Whisper Medium",
    description: "Good without a GPU",
    size_mb: 1500,
    is_downloaded: true,
    speed_score: 0.6,
  }),
  model({
    id: "moonshine-base",
    name: "Moonshine Base",
    description: "Instant, English only",
    size_mb: 58,
    is_downloaded: false,
    supported_languages: ["en"],
    speed_score: 1,
    accuracy_score: 0.4,
  }),
];

const SETTINGS = {
  onboarding_completed: true,
  app_language: "en",
  debug_mode: false,
  post_process_enabled: false,
  selected_model: "whisper-small",
  selected_language: "pt",
  dictation_provider_id: null,
  meeting_provider_id: null,
  fallback_provider_id: null,
};

async function openSettings(page: Page, extra: Record<string, unknown> = {}) {
  const mock = await installTauriMock(page, {
    get_app_settings: SETTINGS,
    get_available_models: MODELS,
    get_current_model: "whisper-small",
    get_model_recommendations: {
      hardware: {
        total_ram_mb: 16000,
        cpu_cores: 8,
        has_avx2: true,
        gpu_names: ["NVIDIA RTX 3060"],
        max_gpu_vram_mb: 12000,
        tier: "gpu",
      },
      default_model_id: "whisper-turbo",
      labels: [{ model_id: "whisper-medium", label: "good_fit" }],
    },
    set_active_model: null,
    download_model: null,
    cancel_download: null,
    delete_model: null,
    "plugin:dialog|ask": true,
    ...extra,
  });
  await page.goto("/");
  await page.getByTitle("Settings").click();
  return mock;
}

const subnav = (page: Page) =>
  page.getByRole("navigation", { name: "Settings sections" });

test.describe("Settings sub-navigation", () => {
  test("lists the four categories and their pages", async ({ page }) => {
    await openSettings(page);
    const nav = subnav(page);
    for (const label of ["Usage", "Transcription", "Intelligence", "App"]) {
      await expect(nav.getByText(label, { exact: true })).toBeVisible();
    }
    for (const page_ of [
      "General",
      "Shortcuts",
      "Microphone & sounds",
      "Models",
      "Languages",
      "API with your own key",
      "Meeting summaries",
      "Assistant",
      "System",
      "Privacy",
      "Advanced",
    ]) {
      await expect(
        nav.getByRole("button", { name: page_, exact: true }),
      ).toBeVisible();
    }
    await expect(
      nav.getByRole("button", { name: "General", exact: true }),
    ).toHaveAttribute("aria-current", "page");
  });

  test("clicking a page shows it with its breadcrumb", async ({ page }) => {
    await openSettings(page);
    await subnav(page).getByRole("button", { name: "Privacy" }).click();
    await expect(
      page.getByRole("heading", { level: 1, name: "Privacy" }),
    ).toBeVisible();
    await expect(page.locator(".st-crumb")).toContainText("App");
    await expect(
      subnav(page).getByRole("button", { name: "Privacy" }),
    ).toHaveAttribute("aria-current", "page");
  });

  for (const [value, button] of [
    ["transcription/models", "Models"],
    ["models", "Models"],
    ["advanced", "Advanced"],
    ["transcription", "Models"],
    ["intelligence/summaries", "Meeting summaries"],
    ["usage/shortcuts", "Shortcuts"],
  ] as const) {
    test(`deep link "${value}" lands on ${button}`, async ({ page }) => {
      const mock = await installTauriMock(page, {
        get_app_settings: SETTINGS,
        get_available_models: MODELS,
        get_current_model: "whisper-small",
      });
      await page.goto("/");
      await expect
        .poll(
          () =>
            mock.calls.filter(
              (c) =>
                c.cmd === "plugin:event|listen" &&
                c.args.event === "hub://navigate",
            ).length,
        )
        .toBeGreaterThanOrEqual(2);
      // The hub is not mounted (we are on Home): the stash must carry it.
      await emitTauriEvent(page, "hub://navigate", {
        section: "settings",
        settingsTab: value,
      });
      await expect(
        subnav(page).getByRole("button", { name: button, exact: true }),
      ).toHaveAttribute("aria-current", "page");
    });
  }

  test("the toast action for summaries lands on Meeting summaries", async ({
    page,
  }) => {
    const mock = await installTauriMock(page, { get_app_settings: SETTINGS });
    await page.goto("/");
    await expect
      .poll(
        () =>
          mock.calls.filter(
            (c) =>
              c.cmd === "plugin:event|listen" &&
              c.args.event === "hub://navigate",
          ).length,
      )
      .toBeGreaterThanOrEqual(2);
    // Same payload the toast's "open_summary_settings" action emits.
    await emitTauriEvent(page, "hub://navigate", {
      section: "settings",
      settingsTab: "intelligence/summaries",
    });
    await expect(
      page.getByRole("heading", { level: 1, name: "Meeting summaries" }),
    ).toBeVisible();
  });
});

test.describe("Transcription → Models", () => {
  test("shows the active model in a hero with metrics and acceleration", async ({
    page,
  }) => {
    await openSettings(page);
    await subnav(page)
      .getByRole("button", { name: "Models", exact: true })
      .click();

    const hero = page.getByTestId("model-hero");
    await expect(
      hero.getByRole("heading", { name: /Whisper Small/ }),
    ).toBeVisible();
    await expect(hero).toContainText("Active");
    await expect(hero).toContainText("466 MB");
    await expect(
      hero.getByRole("img", { name: "Speed: 4 of 5" }),
    ).toBeVisible();
    await expect(hero).toContainText("GPU · NVIDIA RTX 3060");
  });

  test("the table lists the other models with the right action per status", async ({
    page,
  }) => {
    await openSettings(page);
    await subnav(page)
      .getByRole("button", { name: "Models", exact: true })
      .click();

    const table = page.getByRole("table", { name: "Models" });
    await expect(table.getByRole("row")).toHaveCount(4); // header + 3 (active is in the hero)
    const medium = table.locator('tr[data-model-id="whisper-medium"]');
    await expect(medium).toContainText("Good fit");
    await expect(
      medium.getByRole("button", { name: "Use Whisper Medium" }),
    ).toBeVisible();
    const turbo = table.locator('tr[data-model-id="whisper-turbo"]');
    await expect(turbo).toContainText("1.6 GB");
    await expect(turbo).toContainText("Recommended");
    await expect(
      turbo.getByRole("button", { name: "Download Whisper Turbo" }),
    ).toBeVisible();
  });

  test("Use switches the active model", async ({ page }) => {
    const mock = await openSettings(page);
    await subnav(page)
      .getByRole("button", { name: "Models", exact: true })
      .click();
    await page.getByRole("button", { name: "Use Whisper Medium" }).click();
    await expect
      .poll(() => mock.calls.find((c) => c.cmd === "set_active_model"))
      .toMatchObject({ args: { modelId: "whisper-medium" } });
    await expect(
      page
        .getByTestId("model-hero")
        .getByRole("heading", { name: /Whisper Medium/ }),
    ).toBeVisible();
  });

  test("Download shows progress and can be cancelled", async ({ page }) => {
    const mock = await openSettings(page);
    await subnav(page)
      .getByRole("button", { name: "Models", exact: true })
      .click();
    await page.getByRole("button", { name: "Download Whisper Turbo" }).click();
    await expect
      .poll(() => mock.calls.find((c) => c.cmd === "download_model"))
      .toMatchObject({ args: { modelId: "whisper-turbo" } });

    await emitTauriEvent(page, "model-download-progress", {
      model_id: "whisper-turbo",
      downloaded: 64,
      total: 100,
      percentage: 64,
    });
    const row = page.locator('tr[data-model-id="whisper-turbo"]');
    await expect(row.getByRole("progressbar")).toHaveAttribute(
      "aria-valuenow",
      "64",
    );
    await row.getByRole("button", { name: "Cancel download" }).click();
    await expect
      .poll(() => mock.calls.find((c) => c.cmd === "cancel_download"))
      .toMatchObject({ args: { modelId: "whisper-turbo" } });
  });

  test("Delete asks for confirmation and deletes", async ({ page }) => {
    const mock = await openSettings(page);
    await subnav(page)
      .getByRole("button", { name: "Models", exact: true })
      .click();
    await page.getByRole("button", { name: "Delete Whisper Medium" }).click();
    await expect
      .poll(() => mock.calls.find((c) => c.cmd === "delete_model"))
      .toMatchObject({ args: { modelId: "whisper-medium" } });
  });

  test("the quick chips filter the table", async ({ page }) => {
    await openSettings(page);
    await subnav(page)
      .getByRole("button", { name: "Models", exact: true })
      .click();
    const table = page.getByRole("table", { name: "Models" });

    await page.getByRole("button", { name: "My language" }).click();
    await expect(
      table.locator('tr[data-model-id="moonshine-base"]'),
    ).toHaveCount(0);
    await expect(
      table.locator('tr[data-model-id="whisper-turbo"]'),
    ).toHaveCount(1);

    await page.getByRole("button", { name: "Fastest" }).click();
    await expect(
      table.locator('tr[data-model-id="whisper-medium"]'),
    ).toHaveCount(0);
    await expect(
      table.locator('tr[data-model-id="moonshine-base"]'),
    ).toHaveCount(1);

    await page.getByRole("button", { name: "All", exact: true }).click();
    await page.getByRole("searchbox").fill("moon");
    await expect(table.getByRole("row")).toHaveCount(2);
  });

  test("the API option is shown as v1.1+ and disabled", async ({ page }) => {
    await openSettings(page);
    await subnav(page)
      .getByRole("button", { name: "Models", exact: true })
      .click();
    await expect(
      page
        .getByRole("group", { name: "Where to transcribe" })
        .getByRole("button", { name: /API with your own key/ }),
    ).toBeDisabled();

    await subnav(page)
      .getByRole("button", { name: "API with your own key" })
      .click();
    const card = page.getByTestId("api-card");
    await expect(card).toContainText("v1.1+");
    await expect(card.locator("input")).toBeDisabled();
  });
});
