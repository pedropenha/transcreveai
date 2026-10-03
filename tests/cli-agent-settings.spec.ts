import { test, expect } from "@playwright/test";
import { installTauriMock } from "./helpers/tauri-mock";
import {
  startCliSettingsCoverage,
  stopCliSettingsCoverage,
} from "./helpers/cli-settings-coverage";

test.beforeEach(async ({ page }) => startCliSettingsCoverage(page));
test.afterEach(async ({ page }, info) => stopCliSettingsCoverage(page, info));

test("missing experimental can be configured without becoming a selected provider", async ({
  page,
}) => {
  let detected = false;
  let enabled = false;
  let failSave = false;
  const settings = {
    onboarding_completed: true,
    app_language: "en",
    selected_model: "whisper-small",
    assistant_provider_id: null,
    post_process_provider_id: "openai",
    post_process_models: {} as Record<string, string>,
    post_process_providers: [
      {
        id: "openai",
        label: "OpenAI",
        base_url: "https://api.openai.com/v1",
        models_endpoint: "/models",
      },
      {
        id: "cli_agent/cursor_agent",
        label: "Cursor",
        base_url: "",
        models_endpoint: "",
      },
      {
        id: "cli_agent/codex",
        label: "Codex",
        base_url: "",
        models_endpoint: "",
      },
    ],
    cli_agent_configs: {
      "cli_agent/cursor_agent": {
        enabled,
        binary_path: null as string | null,
        extra_args: [],
        timeout_secs: null,
      },
    },
  };
  const mock = await installTauriMock(page, {
    get_app_settings: () => settings,
    cli_agents_status: () => [
      {
        provider_id: "cli_agent/cursor_agent",
        label: "Cursor",
        binary: "cursor-agent",
        detected,
        enabled,
        experimental: true,
        install_hint: "install cursor",
      },
    ],
    cli_agent_update_config: (args: Record<string, unknown>) => {
      if (failSave) throw new Error("invalid private binary path");
      settings.cli_agent_configs["cli_agent/cursor_agent"] =
        args.config as (typeof settings.cli_agent_configs)["cli_agent/cursor_agent"];
      enabled = settings.cli_agent_configs["cli_agent/cursor_agent"].enabled;
      detected =
        !!settings.cli_agent_configs["cli_agent/cursor_agent"].binary_path;
      return null;
    },
    change_post_process_model_setting: (args: Record<string, unknown>) => {
      settings.post_process_models[String(args.providerId)] = String(
        args.model,
      );
      return null;
    },
    set_assistant_provider: (args: Record<string, unknown>) => {
      settings.assistant_provider_id = args.providerId as null;
      return null;
    },
  });
  await page.goto("/");
  await page.getByTitle("Settings").click();
  await page
    .getByRole("navigation", { name: "Settings sections" })
    .getByRole("button", { name: "Assistant", exact: true })
    .click();
  await expect(
    page.getByText("Configure CLI agent", { exact: true }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "Auto (recommended)", exact: true })
    .first()
    .click();
  await expect(
    page
      .getByRole("button", { name: "Cursor — Experimental", exact: true })
      .first(),
  ).toBeDisabled();
  await page
    .getByRole("button", { name: "Auto (recommended)", exact: true })
    .first()
    .click();
  await page
    .getByPlaceholder("Auto-detect on PATH")
    .fill("C:\\tools\\cursor-agent.exe");
  await page.getByPlaceholder("Auto-detect on PATH").blur();
  await expect.poll(() => detected).toBe(true);
  await page
    .getByRole("button", { name: "Auto (recommended)", exact: true })
    .first()
    .click();
  await expect(
    page
      .getByRole("button", { name: "Cursor — Experimental", exact: true })
      .first(),
  ).toBeDisabled();
  await page
    .getByRole("button", { name: "Auto (recommended)", exact: true })
    .first()
    .click();
  await page.getByRole("checkbox", { name: "Enabled", exact: true }).focus();
  await page.keyboard.press("Space");
  await expect.poll(() => enabled).toBe(true);
  await page
    .getByRole("button", { name: "Auto (recommended)", exact: true })
    .first()
    .click();
  await expect(
    page
      .getByRole("button", { name: "Cursor — Experimental", exact: true })
      .first(),
  ).toBeEnabled();
  expect(
    mock.calls.filter(
      (call) =>
        call.cmd === "set_assistant_provider" ||
        call.cmd === "set_post_process_provider",
    ),
  ).toHaveLength(0);
  await page
    .getByRole("button", { name: "Auto (recommended)", exact: true })
    .first()
    .click();
  await page.getByPlaceholder("e.g. --flag value").fill("--model test-model");
  await page.getByPlaceholder("e.g. --flag value").blur();
  await expect
    .poll(() => settings.cli_agent_configs["cli_agent/cursor_agent"].extra_args)
    .toEqual(["--model", "test-model"]);
  await page.getByPlaceholder("60", { exact: true }).fill("90");
  await page.getByPlaceholder("60", { exact: true }).blur();
  await expect
    .poll(
      () => settings.cli_agent_configs["cli_agent/cursor_agent"].timeout_secs,
    )
    .toBe(90);
  await page.getByPlaceholder("CLI default").fill("my-model");
  await page.getByPlaceholder("CLI default").blur();
  await expect
    .poll(() => settings.post_process_models["cli_agent/cursor_agent"])
    .toBe("my-model");
  failSave = true;
  await page
    .getByPlaceholder("Auto-detect on PATH")
    .fill("C:\\private\\bad.exe");
  await page.getByPlaceholder("Auto-detect on PATH").blur();
  await expect(
    page.getByText("Could not save the CLI agent configuration. Try again."),
  ).toBeVisible();
  await expect(page.getByPlaceholder("Auto-detect on PATH")).toHaveValue(
    "C:\\private\\bad.exe",
  );
  await expect(page.getByText("invalid private binary path")).toHaveCount(0);
  await page.evaluate(() => {
    const internals = (
      window as unknown as {
        __TAURI_INTERNALS__: {
          invoke: (command: string, args?: unknown) => Promise<unknown>;
        };
      }
    ).__TAURI_INTERNALS__;
    const original = internals.invoke;
    internals.invoke = (command, args) =>
      command === "cli_agent_update_config"
        ? Promise.reject({ message: "private structured backend error" })
        : original(command, args);
  });
  await page
    .getByPlaceholder("Auto-detect on PATH")
    .fill("C:\\private\\rejected.exe");
  await page.getByPlaceholder("Auto-detect on PATH").blur();
  await expect(
    page.getByText("Could not save the CLI agent configuration. Try again."),
  ).toBeVisible();
  await expect(page.getByPlaceholder("Auto-detect on PATH")).toHaveValue(
    "C:\\private\\rejected.exe",
  );
  await expect(page.getByText("private structured backend error")).toHaveCount(
    0,
  );
  await page
    .getByRole("button", { name: "Cursor — Experimental", exact: true })
    .click();
  await page.getByRole("button", { name: "Codex", exact: true }).click();
  await expect(page.getByPlaceholder("Auto-detect on PATH")).toHaveValue("");
  await expect(
    page.getByText("Could not save the CLI agent configuration. Try again."),
  ).toHaveCount(0);
  await page
    .getByRole("button", { name: "Auto (recommended)", exact: true })
    .first()
    .click();
  await page
    .getByRole("button", { name: "Cursor — Experimental", exact: true })
    .first()
    .click();
  await expect
    .poll(() => settings.assistant_provider_id)
    .toBe("cli_agent/cursor_agent");
});
