import { test, expect, type Page } from "@playwright/test";
import { installTauriMock } from "./helpers/tauri-mock";

const nowSeconds = () => Math.floor(Date.now() / 1000);

function entry(id: number, over: Record<string, unknown> = {}) {
  return {
    id,
    file_name: `dictation-${id}.wav`,
    timestamp: nowSeconds() - id * 60,
    saved: false,
    title: "",
    transcription_text: "",
    post_processed_text: null,
    post_process_prompt: null,
    post_process_requested: false,
    mode: "dictation",
    duration_ms: 4000,
    app_exe: "slack.exe",
    app_name: "Slack",
    stt_provider_id: null,
    llm_provider_id: null,
    language: "pt",
    raw_text: `texto ${id}`,
    final_text: `Texto ditado número ${id}`,
    status: "inserted",
    error_code: null,
    latency_json: "{}",
    audio_available: true,
    word_count: 4,
    ...over,
  };
}

const DAY = 86_400;

const STATS = {
  words_today: 40,
  words_week: 90,
  words_total: 3933,
  duration_ms_total: 1_120_000,
  words_per_minute: 211,
  seconds_saved: 98 * 60,
  provider_usage: [],
};

async function openHome(
  page: Page,
  entries: unknown[],
  extra: Record<string, unknown> = {},
) {
  const mock = await installTauriMock(page, {
    search_history_entries: { entries, has_more: false },
    get_history_statistics: STATS,
    ...extra,
  });
  await page.goto("/");
  return mock;
}

test.describe("Início (history, stats, banner)", () => {
  test("greets, groups by day and shows the stats card", async ({ page }) => {
    await openHome(page, [
      entry(1),
      entry(2),
      entry(3, { timestamp: nowSeconds() - DAY - 3600 }),
      entry(4, { timestamp: nowSeconds() - 5 * DAY }),
    ]);

    await expect(
      page.getByRole("heading", {
        level: 1,
        name: /Good (morning|afternoon|evening)/,
      }),
    ).toBeVisible();
    await expect(page.getByRole("heading", { name: "Today" })).toBeVisible();
    await expect(
      page.getByRole("heading", { name: "Yesterday" }),
    ).toBeVisible();
    await expect(page.getByText("Texto ditado número 1")).toBeVisible();

    const stats = page.getByRole("complementary", {
      name: "Dictation statistics",
    });
    await expect(stats.getByText("3,933")).toBeVisible();
    await expect(stats.getByText("211")).toBeVisible();
    await expect(stats.getByText("≈ 1 h 38 min")).toBeVisible();
    // Streak: today + yesterday are present, the third day is missing => 2.
    await expect(stats.getByText("days in a row")).toBeVisible();
    await expect(stats.getByText("2", { exact: true })).toBeVisible();
    await expect(stats.getByText("No model selected")).toBeVisible();
  });

  test("hover/focus reveals row actions and Copy writes the text", async ({
    page,
    context,
  }) => {
    await context.grantPermissions(["clipboard-read", "clipboard-write"]);
    await openHome(page, [entry(1)]);

    const row = page.locator("#history-entry-1");
    const copy = row.getByRole("button", { name: "Copy transcription" });
    const actions = row.locator(".hist-actions");
    await expect(actions).toHaveCSS("opacity", "0");

    // Keyboard focus is enough to reveal the actions (not hover-only).
    await row.focus();
    await expect(actions).toHaveCSS("opacity", "1");

    await copy.click();
    await expect(page.getByText("Copied to clipboard")).toBeVisible();
    expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(
      "Texto ditado número 1",
    );
  });

  test("failed dictation offers Try again and calls the retry command", async ({
    page,
  }) => {
    const mock = await openHome(page, [
      entry(7, { status: "failed", final_text: "", raw_text: "" }),
    ]);

    await expect(page.getByText("Transcription failed")).toBeVisible();
    await page.getByRole("button", { name: "Try again" }).click();

    await expect
      .poll(
        () =>
          mock.calls.find((c) => c.cmd === "retry_history_entry_transcription")
            ?.args.id,
      )
      .toBe(7);
  });

  test("More opens the detail drawer and Escape closes it", async ({
    page,
  }) => {
    await openHome(page, [entry(1)]);
    await page
      .locator("#history-entry-1")
      .getByRole("button", { name: "More details" })
      .click();

    const drawer = page.getByRole("complementary", {
      name: "Selected dictation details",
    });
    await expect(drawer).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(drawer).toBeHidden();
  });

  test("arrow keys move between rows", async ({ page }) => {
    await openHome(page, [entry(1), entry(2)]);
    await page.locator("#history-entry-1").focus();
    await page.keyboard.press("ArrowDown");
    await expect(page.locator("#history-entry-2")).toBeFocused();
  });

  test("banner dismissal is persisted and its button opens Models", async ({
    page,
  }) => {
    const mock = await openHome(page, []);
    const banner = page.getByTestId("home-banner");
    await expect(banner).toBeVisible();
    await expect(page.getByTestId("home-empty")).toBeVisible();

    await banner.getByRole("button", { name: "Dismiss banner" }).click();
    await expect(banner).toBeHidden();
    expect(
      mock.calls.find((c) => c.cmd === "change_dismissed_ui_setting")?.args.ids,
    ).toEqual(["home_banner"]);
  });

  test("banner CTA navigates to Settings > Models", async ({ page }) => {
    await openHome(page, []);
    await page.getByRole("button", { name: "Choose a model" }).click();
    await expect(
      page.getByRole("button", { name: "Models", exact: true }),
    ).toHaveAttribute("aria-current", "page");
  });

  test("a dismissed banner and tip stay hidden", async ({ page }) => {
    await openHome(page, [], {
      get_app_settings: {
        onboarding_completed: true,
        app_language: "en",
        debug_mode: false,
        post_process_enabled: false,
        show_tray_icon: true,
        dismissed_ui: ["home_banner", "home_tip"],
      },
    });
    await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
    await expect(page.getByTestId("home-banner")).toHaveCount(0);
    await expect(page.getByTestId("home-tip")).toHaveCount(0);
  });

  test("a long history is virtualized", async ({ page }) => {
    const many = Array.from({ length: 300 }, (_, i) => entry(i + 1));
    await openHome(page, many);
    await expect(page.locator("#history-entry-1")).toBeVisible();
    const rendered = await page.locator("article.hist-row").count();
    expect(rendered).toBeGreaterThan(0);
    expect(rendered).toBeLessThan(60);
  });
});
