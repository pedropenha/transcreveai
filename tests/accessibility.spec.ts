import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";
import { installTauriMock } from "./helpers/tauri-mock";

const SETTINGS_PAGES = [
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
] as const;

async function openHub(page: Page, scheme: "light" | "dark") {
  await installTauriMock(page, {
    get_app_settings: {
      onboarding_completed: true,
      app_language: "en",
      debug_mode: false,
      post_process_enabled: false,
      custom_words: ["Transcreve.ai", "Tauri"],
      filler_word_removal_enabled: true,
      selected_language: "pt",
    },
    get_filler_words: ["um", "like"],
    meeting_search: [],
    meeting_current: null,
    meeting_list_templates: [],
    assistant_list_templates: [],
  });
  await page.emulateMedia({ colorScheme: scheme });
  await page.goto("/");
  await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
  await page.evaluate(() => document.fonts.ready);
}

async function expectAccessible(page: Page) {
  // Analyze the whole Hub: no exclusions or disabled rules. Include WCAG 2.2
  // AA as well as axe's best-practice checks for semantic page structure.
  const result = await new AxeBuilder({ page })
    .withTags([
      "wcag2a",
      "wcag2aa",
      "wcag21a",
      "wcag21aa",
      "wcag22aa",
      "best-practice",
    ])
    .analyze();
  await test.info().attach("axe-report", {
    body: JSON.stringify(result.violations),
    contentType: "application/json",
  });
  expect(
    result.violations.map(({ id, impact, nodes }) => ({
      id,
      impact,
      nodes: nodes.map(({ target, failureSummary }) => ({
        target,
        failureSummary,
      })),
    })),
  ).toEqual([]);
}

for (const scheme of ["light", "dark"] as const) {
  test.describe(`Hub accessibility (${scheme})`, () => {
    test.beforeEach(async ({ page }) => {
      await openHub(page, scheme);
    });

    for (const title of ["Home", "Notetaker", "Help"]) {
      test(`${title} including empty states has zero axe violations`, async ({
        page,
      }) => {
        if (title !== "Home")
          await page.getByTitle(title, { exact: true }).click();
        await expect(page.getByTitle(title, { exact: true })).toHaveAttribute(
          "aria-current",
          "page",
        );
        await expectAccessible(page);
      });
    }

    for (const group of ["Vocabulary", "Crutches", "Replacements"]) {
      test(`Dictionary ${group} has zero axe violations`, async ({ page }) => {
        await page.getByTitle("Dictionary", { exact: true }).click();
        await page
          .getByRole("group", { name: "Kind of entry" })
          .getByRole("button", { name: new RegExp(group) })
          .click();
        await expectAccessible(page);
      });
    }

    for (const title of SETTINGS_PAGES) {
      test(`Settings ${title} has zero axe violations`, async ({ page }) => {
        await page.getByTitle("Settings", { exact: true }).click();
        const nav = page.getByRole("navigation", { name: "Settings sections" });
        await nav.getByRole("button", { name: title, exact: true }).click();
        await expect(
          nav.getByRole("button", { name: title, exact: true }),
        ).toHaveAttribute("aria-current", "page");
        await expectAccessible(page);
      });
    }

    test("Ctrl+1..5 reaches every section and Tab/Enter activates dictionary controls", async ({
      page,
    }) => {
      for (const [key, title] of [
        ["2", "Notetaker"],
        ["3", "Dictionary"],
        ["4", "Settings"],
        ["5", "Help"],
        ["1", "Home"],
      ]) {
        await page.keyboard.press(`Control+${key}`);
        await expect(page.getByTitle(title, { exact: true })).toHaveAttribute(
          "aria-current",
          "page",
        );
      }

      await page.keyboard.press("Control+3");
      const vocabulary = page
        .getByRole("group", { name: "Kind of entry" })
        .getByRole("button", { name: /Vocabulary/ });
      await vocabulary.focus();
      await page.keyboard.press("Tab");
      const replacements = page.getByRole("button", { name: /Replacements/ });
      await expect(replacements).toBeFocused();
      await page.keyboard.press("Enter");
      await expect(page.getByTestId("dictionary-replacements")).toBeVisible();
      await page.keyboard.press("Tab");
      const crutches = page.getByRole("button", { name: /Crutches/ });
      await expect(crutches).toBeFocused();
      await page.keyboard.press("Enter");
      await expect(crutches).toHaveAttribute("aria-pressed", "true");
    });

    test("Instrument Sans and Instrument Serif load and are used in the Hub", async ({
      page,
    }) => {
      const fonts = await page.evaluate(async () => {
        await document.fonts.ready;
        const families = Array.from(document.fonts).map((font) => ({
          family: font.family.replaceAll('"', ""),
          status: font.status,
        }));
        return {
          families,
          sans: getComputedStyle(document.querySelector(".rail-item")!)
            .fontFamily,
          serif: getComputedStyle(document.querySelector(".rail-wordmark em")!)
            .fontFamily,
        };
      });
      expect(fonts.families).toContainEqual({
        family: "Instrument Sans Variable",
        status: "loaded",
      });
      expect(fonts.families).toContainEqual({
        family: "Instrument Serif",
        status: "loaded",
      });
      expect(fonts.sans).toContain("Instrument Sans");
      expect(fonts.serif).toContain("Instrument Serif");
    });
  });
}
