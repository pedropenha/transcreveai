import { test, expect, type Page } from "@playwright/test";
import { installTauriMock } from "./helpers/tauri-mock";
import {
  startCorrectionCoverage,
  stopCorrectionCoverage,
} from "./helpers/correction-coverage";

test.beforeEach(async ({ page }) => startCorrectionCoverage(page));
test.afterEach(async ({ page }, info) => stopCorrectionCoverage(page, info));

async function openSummary(
  page: Page,
  outcome: unknown,
  model = "gemini-2.5-flash",
  providerId = "custom",
) {
  let baseUrl = "https://generativelanguage.googleapis.com/v1beta/openai/";
  let cliConfig = {
    enabled: true,
    binary_path: null as string | null,
    extra_args: [],
    timeout_secs: null,
  };
  const mock = await installTauriMock(page, {
    get_app_settings: () => ({
      onboarding_completed: true,
      app_language: "en",
      selected_model: "whisper-small",
      post_process_provider_id: providerId,
      post_process_models: { [providerId]: model },
      cli_agent_configs: { [providerId]: cliConfig },
      post_process_providers: [
        {
          id: providerId,
          label: providerId === "custom" ? "Custom" : "Codex",
          base_url: baseUrl,
          models_endpoint: "/models",
          allow_base_url_edit: true,
        },
      ],
    }),
    test_llm_connection:
      typeof outcome === "function" ? outcome : () => outcome,
    change_post_process_base_url_setting: async (
      args: Record<string, unknown>,
    ) => {
      await new Promise((resolve) => setTimeout(resolve, 300));
      baseUrl = String(args.baseUrl);
      return null;
    },
    cli_agents_status: () => [
      {
        provider_id: providerId,
        label: "Codex",
        binary: "codex",
        detected: true,
        enabled: true,
        experimental: false,
        install_hint: "",
        binary_name: "codex.exe",
      },
    ],
    cli_agent_update_config: async (args: Record<string, unknown>) => {
      await new Promise((resolve) => setTimeout(resolve, 400));
      cliConfig = args.config as typeof cliConfig;
      return null;
    },
  });
  await page.goto("/");
  await page.getByTitle("Settings").click();
  await page
    .getByRole("navigation", { name: "Settings sections" })
    .getByRole("button", { name: "Meeting summaries", exact: true })
    .click();
  return mock;
}

test("tests the selected summary provider and reports model success", async ({
  page,
}) => {
  const mock = await openSummary(page, {
    ok: true,
    provider_id: "custom",
    latency_ms: 150,
  });
  await page.getByRole("button", { name: "Test selected model" }).click();
  await expect(page.getByRole("status")).toContainText(
    "gemini-2.5-flash responded successfully",
  );
  expect(
    mock.calls.filter((call) => call.cmd === "test_llm_connection"),
  ).toEqual([{ cmd: "test_llm_connection", args: { providerId: "custom" } }]);
});

for (const [kind, message] of [
  ["auth", "API key"],
  ["rate_limited", "quota"],
  ["provider", "model identifier"],
  ["timeout", "time"],
  ["offline", "Offline"],
]) {
  test(`shows a localized ${kind} failure without raw provider details`, async ({
    page,
  }) => {
    await openSummary(page, {
      ok: false,
      provider_id: "custom",
      kind,
      detail: "private diagnostic must not appear",
    });
    await page.getByRole("button", { name: "Test selected model" }).click();
    await expect(page.getByRole("status")).toContainText(message);
    await expect(
      page.getByText("private diagnostic must not appear"),
    ).toHaveCount(0);
  });
}

test("an empty model cannot be tested", async ({ page }) => {
  await openSummary(page, null, "");
  await expect(
    page.getByRole("button", { name: "Test selected model" }),
  ).toBeDisabled();
});

test("blocks duplicate presses while the selected model is being tested", async ({
  page,
}) => {
  const mock = await openSummary(page, async () => {
    await new Promise((resolve) => setTimeout(resolve, 600));
    return { ok: true, provider_id: "custom", latency_ms: 600 };
  });
  await page.getByRole("button", { name: "Test selected model" }).click();
  await expect(page.getByRole("button", { name: "Testing…" })).toBeDisabled();
  await expect(page.getByRole("status")).toContainText(
    "responded successfully",
  );
  expect(
    mock.calls.filter((call) => call.cmd === "test_llm_connection"),
  ).toHaveLength(1);
});

test("a command rejection restores the test button with a safe error", async ({
  page,
}) => {
  await openSummary(page, () => {
    throw new Error("private path and key");
  });
  await page.getByRole("button", { name: "Test selected model" }).click();
  await expect(page.getByRole("status")).toContainText("model identifier");
  await expect(
    page.getByRole("button", { name: "Test selected model" }),
  ).toBeEnabled();
  await expect(page.getByText("private path and key")).toHaveCount(0);
});

test("ignores a late result after leaving the summary settings", async ({
  page,
}) => {
  await openSummary(page, async () => {
    await new Promise((resolve) => setTimeout(resolve, 600));
    return { ok: true, provider_id: "custom", latency_ms: 600 };
  });
  await page.getByRole("button", { name: "Test selected model" }).click();
  const sections = page.getByRole("navigation", { name: "Settings sections" });
  await sections
    .getByRole("button", { name: "Assistant", exact: true })
    .click();
  await page.waitForTimeout(700);
  await sections
    .getByRole("button", { name: "Meeting summaries", exact: true })
    .click();
  await expect(page.getByRole("status")).toBeEmpty();
  await expect(
    page.getByRole("button", { name: "Test selected model" }),
  ).toBeEnabled();
});

test("waits for configuration saving and discards an earlier test result", async ({
  page,
}) => {
  const mock = await openSummary(page, async () => {
    await new Promise((resolve) => setTimeout(resolve, 700));
    return { ok: true, provider_id: "custom", latency_ms: 700 };
  });
  await page.getByRole("button", { name: "Test selected model" }).click();
  const url = page.getByPlaceholder("https://api.openai.com/v1");
  await url.fill("https://example.com/v1");
  await url.blur();
  await expect(
    page.getByRole("button", { name: "Test selected model" }),
  ).toBeDisabled();
  await expect(
    page.getByRole("button", { name: "Test selected model" }),
  ).toBeEnabled();
  await page.waitForTimeout(750);
  await expect(page.getByRole("status")).toBeEmpty();
  expect(
    mock.calls.filter((call) => call.cmd === "test_llm_connection"),
  ).toHaveLength(1);
});

test("waits for a CLI binary-path change to finish saving before testing", async ({
  page,
}) => {
  const mock = await openSummary(
    page,
    { ok: true, provider_id: "cli_agent/codex", latency_ms: 100 },
    "",
    "cli_agent/codex",
  );
  const path = page.getByPlaceholder("Auto-detect on PATH");
  await path.fill("C:\\tools\\codex.exe");
  await path.blur();
  await expect(
    page.getByRole("button", { name: "Test selected model" }),
  ).toBeDisabled();
  await expect(
    page.getByRole("button", { name: "Test selected model" }),
  ).toBeEnabled();
  expect(
    mock.calls.filter((call) => call.cmd === "test_llm_connection"),
  ).toHaveLength(0);
  await page.getByRole("button", { name: "Test selected model" }).click();
  await expect(page.getByRole("status")).toContainText(
    "default model responded successfully",
  );
});
