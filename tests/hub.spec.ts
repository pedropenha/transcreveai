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
});
