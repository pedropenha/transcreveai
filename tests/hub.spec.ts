import { test, expect } from "@playwright/test";
import { emitTauriEvent, installTauriMock } from "./helpers/tauri-mock";

test.describe("hub webview (mocked Tauri IPC)", () => {
  test("returning user lands on the settings UI", async ({ page }) => {
    const mock = await installTauriMock(page);
    await page.goto("/");

    // With onboarding_completed=true the app renders the hub: the sidebar
    // lists the always-on sections (sidebar items carry their label in
    // `title`; the content area shows a heading).
    await expect(page.getByTitle("General")).toBeVisible();
    await expect(page.getByTitle("Advanced")).toBeVisible();
    await expect(page.getByTitle("About")).toBeVisible();
    await expect(page.getByRole("heading", { name: "General" })).toBeVisible();

    // The IPC mock actually got exercised by startup.
    expect(mock.calls.map((c) => c.cmd)).toContain("get_app_settings");
  });

  test("clicking a sidebar section switches the content", async ({ page }) => {
    await installTauriMock(page);
    await page.goto("/");
    await expect(page.getByTitle("General")).toBeVisible();

    await page.getByTitle("About").click();
    // AboutSettings shows the app name/version block.
    await expect(page.getByText("Transcreve.ai").first()).toBeVisible();
  });

  test("backend recording-error event surfaces a toast", async ({ page }) => {
    const mock = await installTauriMock(page);
    await page.goto("/");
    await expect(page.getByTitle("General")).toBeVisible();
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
