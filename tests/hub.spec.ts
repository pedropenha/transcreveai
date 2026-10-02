import { test, expect } from "@playwright/test";
import { emitTauriEvent, installTauriMock } from "./helpers/tauri-mock";

test.describe("hub webview (mocked Tauri IPC)", () => {
  test("returning user lands on the settings UI", async ({ page }) => {
    const mock = await installTauriMock(page);
    await page.goto("/");

    // With onboarding_completed=true the app renders the Hub's v1 information
    // architecture and lands on the history view.
    await expect(page.getByTitle("Home", { exact: true })).toBeVisible();
    await expect(page.getByTitle("Notetaker")).toBeVisible();
    await expect(page.getByTitle("Dictionary")).toBeVisible();
    await expect(page.getByTitle("Settings")).toBeVisible();
    await expect(page.getByTitle("Help")).toBeVisible();
    // "Models" left the rail: it now lives under Settings.
    await expect(
      page
        .getByRole("navigation", { name: "Hub sections" })
        .getByTitle("Models"),
    ).toHaveCount(0);
    await expect(
      page.getByRole("heading", {
        level: 1,
        name: /Good (morning|afternoon|evening)/,
      }),
    ).toBeVisible();

    // The IPC mock actually got exercised by startup.
    expect(mock.calls.map((c) => c.cmd)).toContain("get_app_settings");
  });

  test("clicking a sidebar section switches the content", async ({ page }) => {
    await installTauriMock(page);
    await page.goto("/");
    await expect(page.getByTitle("Home", { exact: true })).toBeVisible();

    await page.getByTitle("Dictionary").click();
    await expect(
      page.getByRole("heading", { name: "Dictionary" }),
    ).toBeVisible();
  });

  test("backend recording-error event surfaces a toast", async ({ page }) => {
    const mock = await installTauriMock(page);
    await page.goto("/");
    await expect(page.getByTitle("Home", { exact: true })).toBeVisible();
    // Wait for the app's useEffects to have registered the event listeners.
    await expect
      .poll(
        () => mock.calls.filter((c) => c.cmd === "plugin:event|listen").length,
        { timeout: 10_000 },
      )
      .toBeGreaterThan(0);

    await emitTauriEvent(page, "recording-error", {
      error_type: "no_input_device",
      detail: null,
    });

    await expect(page.getByText("No Microphone Found")).toBeVisible();
  });

  test("meeting state events refresh the Meetings list immediately", async ({
    page,
  }) => {
    const meeting = {
      id: "meeting-1",
      title: "Sprint sync",
      app_exe: "chrome.exe",
      app_label: "Google Meet",
      detection: "auto_prompt",
      status: "processing",
      started_at: 1_700_000_000,
      ended_at: 1_700_000_600,
      capture_system_audio: true,
      stt_provider_id: null,
      llm_provider_id: null,
      template_id: null,
      summary_md: null,
      summary_status: "pending",
      audio_dir: null,
      language: "en",
      error_code: null,
      source_app: { exe: "chrome.exe", name: "Google Meet" },
      list_status: "processing",
    };
    let rows: unknown[] = [];
    const mock = await installTauriMock(page, {
      meeting_search: () => rows,
    });
    await page.goto("/");
    await page.getByTitle("Notetaker").click();
    await expect(page.getByText("No meetings recorded yet")).toBeVisible();
    await expect
      .poll(() =>
        mock.calls.some(
          (call) =>
            call.cmd === "plugin:event|listen" &&
            call.args.event === "meeting://state",
        ),
      )
      .toBe(true);

    // Backend order is deliberately: persist lifecycle, emit the event, then
    // the Hub reloads — the event itself is the refresh trigger.
    rows = [meeting];
    await emitTauriEvent(page, "meeting://state", {
      meeting_id: meeting.id,
      status: "processing",
      elapsed_ms: 600_000,
    });

    await expect(
      page.getByRole("button", { name: /Sprint sync/ }),
    ).toBeVisible();
    await expect(page.getByText("Transcribing").first()).toBeVisible();
  });
});

test.describe("hub shell (Papel & Anil)", () => {
  test("Ctrl+1..5 and Ctrl+, switch sections", async ({ page }) => {
    await installTauriMock(page);
    await page.goto("/");
    await expect(
      page.getByRole("heading", {
        level: 1,
        name: /Good (morning|afternoon|evening)/,
      }),
    ).toBeVisible();

    await page.keyboard.press("Control+3");
    await expect(
      page.getByRole("heading", { name: "Dictionary" }),
    ).toBeVisible();
    await expect(page.getByTitle("Dictionary")).toHaveAttribute(
      "aria-current",
      "page",
    );

    await page.keyboard.press("Control+5");
    await expect(page.getByRole("heading", { name: "Help" })).toBeVisible();

    await page.keyboard.press("Control+Comma");
    await expect(
      page.getByRole("navigation", { name: "Settings sections" }),
    ).toBeVisible();

    await page.keyboard.press("Control+1");
    await expect(
      page.getByRole("heading", {
        level: 1,
        name: /Good (morning|afternoon|evening)/,
      }),
    ).toBeVisible();
  });

  test("legacy section 'models' lands on Settings > Models", async ({
    page,
  }) => {
    const mock = await installTauriMock(page);
    await page.goto("/");
    // Two listeners: pendingSettingsTab (module load) and App (effect).
    await expect
      .poll(
        () =>
          mock.calls.filter(
            (call) =>
              call.cmd === "plugin:event|listen" &&
              call.args.event === "hub://navigate",
          ).length,
      )
      .toBeGreaterThanOrEqual(2);

    await emitTauriEvent(page, "hub://navigate", { section: "models" });

    await expect(
      page.getByRole("button", { name: "Models", exact: true }),
    ).toHaveAttribute("aria-current", "page");
  });

  test("v1.1 entries are disabled and never navigate", async ({ page }) => {
    await installTauriMock(page);
    await page.goto("/");
    const notes = page.getByTitle("Notes · Coming in v1.1");
    await expect(notes).toHaveAttribute("aria-disabled", "true");
    await notes.click({ force: true });
    await expect(
      page.getByRole("heading", {
        level: 1,
        name: /Good (morning|afternoon|evening)/,
      }),
    ).toBeVisible();
  });

  test("rail collapses to icons and remembers the choice", async ({ page }) => {
    await installTauriMock(page);
    await page.goto("/");
    const rail = page.getByRole("navigation", { name: "Hub sections" });
    await expect(rail).toHaveAttribute("data-collapsed", "false");
    expect((await rail.boundingBox())?.width).toBeGreaterThan(200);

    await page.getByRole("button", { name: "Collapse sidebar" }).click();
    await expect(rail).toHaveAttribute("data-collapsed", "true");
    await expect.poll(async () => (await rail.boundingBox())?.width).toBe(64);

    await page.reload();
    await expect(rail).toHaveAttribute("data-collapsed", "true");
  });

  test("setup checklist can be dismissed and the choice is persisted", async ({
    page,
  }) => {
    const mock = await installTauriMock(page);
    await page.goto("/");
    const card = page.getByTestId("setup-checklist");
    await expect(card).toBeVisible();
    await expect(card.getByRole("progressbar")).toHaveAttribute(
      "aria-valuenow",
      "0",
    );

    await card.getByRole("button", { name: "Dismiss checklist" }).click();

    await expect(card).toBeHidden();
    const write = mock.calls.find(
      (call) => call.cmd === "change_dismissed_ui_setting",
    );
    expect(write?.args.ids).toEqual(["setup_checklist"]);
  });

  test("a checklist item opens its settings tab and is remembered", async ({
    page,
  }) => {
    const mock = await installTauriMock(page);
    await page.goto("/");
    await page
      .getByTestId("setup-checklist")
      .getByRole("button", { name: "Test the microphone" })
      .click();

    await expect(
      page.getByRole("navigation", { name: "Settings sections" }),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Microphone & sounds", exact: true }),
    ).toHaveAttribute("aria-current", "page");
    expect(
      mock.calls.find((call) => call.cmd === "change_dismissed_ui_setting")
        ?.args.ids,
    ).toEqual(["setup:microphone"]);
  });

  test("an already-dismissed checklist stays hidden after reload", async ({
    page,
  }) => {
    await installTauriMock(page, {
      get_app_settings: {
        onboarding_completed: true,
        app_language: "en",
        debug_mode: false,
        post_process_enabled: false,
        show_tray_icon: true,
        dismissed_ui: ["setup_checklist"],
      },
    });
    await page.goto("/");
    await expect(
      page.getByRole("heading", {
        level: 1,
        name: /Good (morning|afternoon|evening)/,
      }),
    ).toBeVisible();
    await expect(page.getByTestId("setup-checklist")).toHaveCount(0);
  });
});
