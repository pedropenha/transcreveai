import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";
import { emitTauriEvent, installTauriMock } from "./helpers/tauri-mock";
import type { CommandHandlers } from "./helpers/tauri-mock";
import { mkdir, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import {
  startCorrectionCoverage,
  stopCorrectionCoverage,
} from "./helpers/correction-coverage";

test.beforeEach(async ({ page }) => {
  await startCorrectionCoverage(page);
  if (process.env.TRANSLATION_COVERAGE === "1") {
    await page.coverage.startJSCoverage({ resetOnNavigation: false });
  }
});
test.afterEach(async ({ page }, testInfo) => {
  await stopCorrectionCoverage(page, testInfo);
  if (process.env.TRANSLATION_COVERAGE === "1") {
    const coverage = await page.coverage.stopJSCoverage();
    const directory = resolve("coverage/translated-dictation");
    await mkdir(directory, { recursive: true });
    await writeFile(
      resolve(directory, `${testInfo.testId}-${testInfo.retry}.json`),
      JSON.stringify(coverage),
    );
  }
});

const model = (id: string, supports: boolean, downloaded = true) => ({
  id,
  name: id,
  description: id,
  filename: `${id}.bin`,
  source: { HuggingFace: { repo_id: "whisper", revision: "main" } },
  size_mb: 1500,
  is_downloaded: downloaded,
  is_downloading: false,
  partial_size: 0,
  is_directory: false,
  engine_type: "TranscribeCpp",
  accuracy_score: 0.8,
  speed_score: 0.6,
  supports_translation: supports,
  supported_languages: ["en", "pt"],
  supports_language_selection: true,
  supports_language_detection: true,
  supports_streaming: false,
  is_custom: false,
  is_recommended: false,
});
const models = [
  model("Whisper Turbo", false),
  model("Whisper Medium", true),
  model("Whisper Large", true, false),
];
const bindings = {
  transcribe_translate: {
    id: "transcribe_translate",
    name: "Translate dictation hotkey",
    description: "",
    default_binding: "ctrl+alt+space",
    current_binding: "ctrl+alt+space",
  },
};

async function open(
  page: Page,
  selected: string | null = null,
  language = "en",
  extra: CommandHandlers = {},
) {
  const catalog = models.map((model) => ({ ...model }));
  const mock = await installTauriMock(page, {
    get_app_settings: {
      onboarding_completed: true,
      app_language: language,
      selected_model: "Whisper Turbo",
      translation_model_id: selected,
      selected_language: "pt",
      translate_to_english: false,
      bindings,
    },
    get_available_models: () => catalog,
    get_current_model: "Whisper Turbo",
    change_translation_model_setting: null,
    download_model: null,
    cancel_download: null,
    ...extra,
  });
  await page.goto("/");
  await page
    .getByTitle(language === "en" ? "Settings" : "Configurações")
    .click();
  await page
    .getByRole("navigation", {
      name:
        language === "en" ? "Settings sections" : "Seções das configurações",
    })
    .getByRole("button", {
      name: language === "en" ? "Languages" : "Idiomas",
      exact: true,
    })
    .click();
  return {
    ...mock,
    completeDownload: () => {
      catalog[2].is_downloaded = true;
    },
  };
}

test("warns, with an icon, that the current model can't translate and explains the list", async ({
  page,
}) => {
  await open(page);
  const card = page.getByTestId("translated-dictation-settings");

  const status = card.getByRole("status");
  await expect(status).toHaveAttribute("data-tone", "warning");
  await expect(status.locator("svg")).toHaveAttribute("aria-hidden", "true");

  const select = card.getByLabel("Translation model");
  await expect(select.getByRole("option").first()).toHaveText(
    "Current model (Whisper Turbo) — can't translate",
  );
  await expect(
    select.getByRole("option", { name: "Whisper Medium — downloaded" }),
  ).toHaveCount(1);
  await expect(
    select.getByRole("option", { name: "Whisper Large — not downloaded" }),
  ).toHaveCount(1);
  await expect(card.getByText(/Only models that can translate/)).toBeVisible();
  if (process.env.CAPTURE_SCREENS === "1") {
    await card.screenshot({
      path: "docs/design/screens/translation-model-warning.png",
    });
  }

  await select.selectOption("Whisper Medium");
  await expect(status).toHaveAttribute("data-tone", "success");
});

test("chooses a compatible session model without changing ordinary dictation", async ({
  page,
}) => {
  const mock = await open(page);
  const card = page.getByTestId("translated-dictation-settings");
  await expect(card.getByRole("status")).toContainText(
    "Choose a compatible local model",
  );
  const select = card.getByLabel("Translation model");
  await expect(select.getByRole("option")).toHaveCount(3);
  await select.selectOption("Whisper Medium");
  await expect(card.getByRole("status")).toContainText("Ready");
  expect(
    mock.calls
      .filter((c) => c.cmd === "change_translation_model_setting")
      .map((c) => c.args),
  ).toEqual([{ modelId: "Whisper Medium" }]);
  expect(
    mock.calls.some((c) =>
      [
        "set_active_model",
        "change_translate_to_english_setting",
        "set_dictation_provider",
      ].includes(c.cmd),
    ),
  ).toBe(false);
  await expect(
    card.getByText("Ctrl + Alt + Space", { exact: true }),
  ).toBeVisible();
  const audit = await new AxeBuilder({ page })
    .include('[data-testid="translated-dictation-settings"]')
    .analyze();
  expect(audit.violations).toEqual([]);
});

test("downloads only on explicit request, reports readiness, and allows inheritance", async ({
  page,
}) => {
  const mock = await open(page, "Whisper Large");
  const card = page.getByTestId("translated-dictation-settings");
  await expect(card.getByRole("status")).toContainText(
    "Download the selected model",
  );
  expect(mock.calls.some((c) => c.cmd === "download_model")).toBe(false);
  await card.getByRole("button", { name: "Download model" }).click();
  await expect(card.getByRole("status")).toContainText("Downloading");
  expect(
    mock.calls.filter((c) => c.cmd === "download_model").map((c) => c.args),
  ).toEqual([{ modelId: "Whisper Large" }]);
  mock.completeDownload();
  await emitTauriEvent(page, "model-download-complete", "Whisper Large");
  await expect(card.getByRole("status")).toContainText("Ready");
  await card.getByLabel("Translation model").selectOption("");
  expect(
    mock.calls
      .filter((c) => c.cmd === "change_translation_model_setting")
      .map((c) => c.args),
  ).toEqual([{ modelId: null }]);
});

test("configures the model by keyboard in Brazilian Portuguese", async ({
  page,
}) => {
  const mock = await open(page, null, "pt-BR");
  const card = page.getByTestId("translated-dictation-settings");
  await expect(
    card.getByRole("heading", { name: "Traduzir este ditado" }),
  ).toBeVisible();
  const select = card.getByLabel("Modelo para tradução");
  await select.focus();
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("Enter");
  await expect(card.getByRole("status")).toContainText(
    "Pronto: Whisper Medium",
  );
  await expect(
    card.getByText("Atalho de ditado traduzido", { exact: true }),
  ).toBeVisible();
  await expect
    .poll(
      () =>
        mock.calls.filter(
          (call) => call.cmd === "change_translation_model_setting",
        ).length,
    )
    .toBeGreaterThanOrEqual(1);
});

test("preserves and explains a unavailable saved model until explicitly changed", async ({
  page,
}) => {
  await open(page, "missing-model");
  const card = page.getByTestId("translated-dictation-settings");
  await expect(card.getByLabel("Translation model")).toHaveValue(
    "missing-model",
  );
  await expect(card.getByRole("status")).toContainText(
    "Choose a compatible local model",
  );
  await card.getByLabel("Translation model").selectOption("Whisper Medium");
  await expect(card.getByRole("status")).toContainText("Ready");
});

test("reports a failed save and rolls the selector back", async ({ page }) => {
  await open(page, null, "en", {
    change_translation_model_setting: () => {
      throw new Error("private path");
    },
  });
  const card = page.getByTestId("translated-dictation-settings");
  await card.getByLabel("Translation model").selectOption("Whisper Medium");
  await expect(
    page.getByText("Could not save the translation model. Try again.", {
      exact: true,
    }),
  ).toBeVisible();
  await expect(card.getByLabel("Translation model")).toHaveValue("");
  await expect(page.getByText("private path", { exact: true })).toHaveCount(0);
});

test("offers retry after a failed download without technical error text", async ({
  page,
}) => {
  await open(page, "Whisper Large", "en", {
    download_model: () => {
      throw new Error("private path");
    },
  });
  const card = page.getByTestId("translated-dictation-settings");
  await card.getByRole("button", { name: "Download model" }).click();
  await expect(card.getByRole("alert")).toContainText(
    "Could not download or cancel the model",
  );
  await expect(
    card.getByRole("button", { name: "Download model" }),
  ).toBeVisible();
  await expect(page.getByText("private path", { exact: true })).toHaveCount(0);
});

test("cancels the explicitly requested download", async ({ page }) => {
  const mock = await open(page, "Whisper Large");
  const card = page.getByTestId("translated-dictation-settings");
  await card.getByRole("button", { name: "Download model" }).click();
  await card.getByRole("button", { name: "Cancel download" }).click();
  await expect(card.getByRole("status")).toContainText(
    "Download the selected model",
  );
  expect(
    mock.calls
      .filter((call) => call.cmd === "cancel_download")
      .map((call) => call.args),
  ).toEqual([{ modelId: "Whisper Large" }]);
});

test("Flowbar indicates translation only for the translation session", async ({
  page,
}) => {
  const mock = await installTauriMock(page, {
    get_app_settings: { app_language: "en", flowbar_visibility: "always" },
  });
  await page.goto("/src/overlay/index.html");
  await expect
    .poll(() =>
      mock.calls.some(
        (call) =>
          call.cmd === "plugin:event|listen" &&
          call.args.event === "session://state",
      ),
    )
    .toBe(true);
  await emitTauriEvent(page, "show-overlay", "recording");
  await emitTauriEvent(page, "session://state", {
    session_id: "translation",
    state: "recording",
    mode: "translation",
    pending: 0,
  });
  await expect(
    page.getByLabel("English translation", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText("Listening for English translation", { exact: true }),
  ).toBeAttached();
  await emitTauriEvent(page, "session://state", {
    session_id: "translation",
    state: "transcribing",
    mode: "translation",
    pending: 0,
  });
  await expect(
    page.getByText("Translating into English", { exact: true }),
  ).toBeAttached();
  await emitTauriEvent(page, "session://state", {
    session_id: "normal",
    state: "recording",
    mode: "dictation",
    pending: 0,
  });
  await expect(
    page.getByLabel("English translation", { exact: true }),
  ).toHaveCount(0);
});

test("Hub avoids duplicating floating translation errors and still shows microphone errors", async ({
  page,
}) => {
  const mock = await open(page);
  await expect
    .poll(() =>
      mock.calls.some(
        (call) =>
          call.cmd === "plugin:event|listen" &&
          call.args.event === "recording-error",
      ),
    )
    .toBe(true);
  await emitTauriEvent(page, "recording-error", {
    error_type: "no_input_device",
    detail: "internal path secret",
  });
  await expect(page.locator("[data-sonner-toast]")).toHaveCount(1);
  await emitTauriEvent(page, "recording-error", {
    error_type: "translation_model_unavailable",
    detail: "internal path secret",
  });
  // Sonner defers notifications; settle its scheduled render before checking
  // absence (an immediate negative assertion would pass before mounting).
  await page.waitForTimeout(300);
  // Translation errors already have a non-activating native notice, including
  // when the Hub is visible. The Hub should not create a second Sonner toast.
  await expect(page.locator("[data-sonner-toast]")).toHaveCount(1);
  await expect(page.getByText("internal path secret")).toHaveCount(0);
});

for (const [language, title, body] of [
  [
    "en",
    "Recording could not start",
    "Choose a compatible local translation model in Settings → Transcription → Languages.",
  ],
  [
    "pt-BR",
    "Não foi possível iniciar a gravação",
    "Escolha um modelo local compatível em Configurações → Transcrição → Idiomas.",
  ],
] as const) {
  test(`floating translation notice is localized without opening the Hub (${language})`, async ({
    page,
  }) => {
    const mock = await installTauriMock(page, {
      get_app_settings: { app_language: language },
    });
    await page.goto("/src/toast/index.html");
    await expect
      .poll(() =>
        mock.calls.some(
          (call) =>
            call.cmd === "plugin:event|listen" &&
            call.args.event === "toast://state",
        ),
      )
      .toBe(true);
    await emitTauriEvent(page, "toast://state", {
      collapsed: false,
      detection: null,
      notice: {
        kind: "translated_dictation_error",
        message: "translation_model_required",
        action: null,
        meeting_id: null,
      },
    });
    await expect(
      page.getByRole("heading", { name: title, exact: true }),
    ).toBeVisible();
    await expect(page.locator(".tnotice-msg")).toContainText(body);
    await expect(page.locator('[aria-live="polite"]')).toContainText(body);
    expect(
      mock.calls.some((call) =>
        /show_main|show_hub|set_focus|hub.*navigate/.test(call.cmd),
      ),
    ).toBe(false);
    expect(
      mock.calls.some(
        (call) =>
          call.cmd === "plugin:event|emit" &&
          call.args.event === "hub://navigate",
      ),
    ).toBe(false);
  });
}

test("floating translation notice hides unknown technical details and preserves ordinary notices", async ({
  page,
}) => {
  const mock = await installTauriMock(page, {
    get_app_settings: { app_language: "en" },
  });
  await page.goto("/src/toast/index.html");
  await expect
    .poll(() =>
      mock.calls.some(
        (call) =>
          call.cmd === "plugin:event|listen" &&
          call.args.event === "toast://state",
      ),
    )
    .toBe(true);
  await emitTauriEvent(page, "toast://state", {
    collapsed: false,
    detection: null,
    notice: {
      kind: "translated_dictation_error",
      message: "private internal path",
      action: null,
    },
  });
  await expect(page.locator(".tnotice-msg")).toContainText(
    "Translation could not finish. Try recording again.",
  );
  await expect(
    page.getByText("private internal path", { exact: true }),
  ).toHaveCount(0);
  await emitTauriEvent(page, "toast://state", {
    collapsed: false,
    detection: null,
    notice: {
      kind: "warning",
      message: "Ordinary notice unchanged",
      action: null,
    },
  });
  await expect(page.locator(".tnotice-msg")).toHaveText(
    "Ordinary notice unchanged",
  );
});
