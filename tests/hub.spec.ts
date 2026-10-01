import { test, expect } from "@playwright/test";
import { emitTauriEvent, installTauriMock } from "./helpers/tauri-mock";

test.describe("hub webview (mocked Tauri IPC)", () => {
  test("returning user lands on the settings UI", async ({ page }) => {
    const mock = await installTauriMock(page);
    await page.goto("/");

    // With onboarding_completed=true the app renders the Hub's v1 information
    // architecture and lands on the history view.
    await expect(page.getByTitle("Home")).toBeVisible();
    await expect(page.getByTitle("Meetings")).toBeVisible();
    await expect(page.getByTitle("Dictionary")).toBeVisible();
    await expect(page.getByTitle("Models")).toBeVisible();
    await expect(page.getByTitle("Settings")).toBeVisible();
    await expect(page.getByRole("heading", { name: "History" })).toBeVisible();

    // The IPC mock actually got exercised by startup.
    expect(mock.calls.map((c) => c.cmd)).toContain("get_app_settings");
  });

  test("clicking a sidebar section switches the content", async ({ page }) => {
    await installTauriMock(page);
    await page.goto("/");
    await expect(page.getByTitle("Home")).toBeVisible();

    await page.getByTitle("Dictionary").click();
    await expect(
      page.getByRole("heading", { name: "Dictionary" }),
    ).toBeVisible();
  });

  test("backend recording-error event surfaces a toast", async ({ page }) => {
    const mock = await installTauriMock(page);
    await page.goto("/");
    await expect(page.getByTitle("Home")).toBeVisible();
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
    };
    let rows: unknown[] = [];
    const mock = await installTauriMock(page, {
      meeting_search: () => rows,
    });
    await page.goto("/");
    await page.getByTitle("Meetings").click();
    await expect(page.getByText("No meetings yet")).toBeVisible();
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
      page.getByRole("button", { name: "Open meeting Sprint sync" }),
    ).toBeVisible();
    await expect(page.getByText("Processing")).toBeVisible();
  });
});
