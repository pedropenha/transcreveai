import { test, expect, type Page } from "@playwright/test";
import { installTauriMock } from "./helpers/tauri-mock";
import {
  startDictionaryCoverage,
  stopDictionaryCoverage,
} from "./helpers/dictionary-coverage";

test.beforeEach(async ({ page }) => {
  await startDictionaryCoverage(page);
});
test.afterEach(async ({ page }, testInfo) => {
  await stopDictionaryCoverage(page, testInfo);
});

const SETTINGS = {
  onboarding_completed: true,
  app_language: "en",
  debug_mode: false,
  post_process_enabled: false,
  custom_words: ["Transcreve.ai", "Tauri"],
  filler_word_removal_enabled: true,
};

async function openDictionary(page: Page, fillers = ["um", "like"]) {
  const mock = await installTauriMock(page, {
    get_app_settings: SETTINGS,
    get_filler_words: fillers,
    set_filler_words: null,
    reset_filler_words: null,
    change_filler_word_removal_enabled_setting: null,
    update_custom_words: null,
  });
  await page.goto("/");
  await page.getByTitle("Dictionary").click();
  await expect(
    page.getByRole("heading", { level: 1, name: "Dictionary" }),
  ).toBeVisible();
  return mock;
}

const tabs = (page: Page) => page.getByRole("group", { name: "Kind of entry" });

test.describe("Dictionary (three tabs + preview)", () => {
  test("desktop dictionary uses the proposal dimensions and serif preview heading", async ({
    page,
  }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await openDictionary(page);
    const dimensions = await page
      .locator(".dict-explain")
      .evaluate((element) => {
        const row = document.querySelector(".dict-list .dict-row");
        const heading = element.querySelector("h2");
        if (!row || !heading) throw new Error("Dictionary content missing");
        return {
          width: element.getBoundingClientRect().width,
          gap: getComputedStyle(element.parentElement!).columnGap,
          padding: getComputedStyle(row).padding,
          font: getComputedStyle(heading).fontFamily,
          size: getComputedStyle(heading).fontSize,
        };
      });
    expect(dimensions).toMatchObject({
      width: 280,
      gap: "28px",
      padding: "12px 16px",
      size: "24px",
    });
    expect(dimensions.font).toContain("Instrument Serif");
  });
  test("load errors block crutch edits until a successful retry", async ({
    page,
  }) => {
    let available = false;
    await installTauriMock(page, {
      get_app_settings: SETTINGS,
      get_filler_words: () => {
        if (!available) throw new Error("load unavailable");
        return ["um"];
      },
    });
    await page.goto("/");
    await page.getByTitle("Dictionary").click();
    await tabs(page)
      .getByRole("button", { name: /Crutches/ })
      .click();
    await expect(page.getByLabel("New filler word or phrase")).toBeDisabled();
    available = true;
    await page.getByRole("button", { name: "Try again", exact: true }).click();
    await expect(page.getByRole("button", { name: "Remove um" })).toBeVisible();
    await expect(page.getByLabel("New filler word or phrase")).toBeEnabled();
  });

  test("failed vocabulary writes preserve the draft and existing terms", async ({
    page,
  }) => {
    await installTauriMock(page, {
      get_app_settings: SETTINGS,
      update_custom_words: () => {
        throw new Error("write unavailable");
      },
    });
    await page.goto("/");
    await page.getByTitle("Dictionary").click();
    await page.getByLabel("New vocabulary term").fill("New term");
    await page.getByRole("button", { name: "Add", exact: true }).click();
    await expect(page.getByLabel("New vocabulary term")).toHaveValue(
      "New term",
    );
    await expect(
      page.getByText(
        "Could not save vocabulary. Your changes were kept for retry.",
      ),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Remove New term" }),
    ).toHaveCount(0);
  });

  test("duplicates cannot be added and vocabulary can be removed", async ({
    page,
  }) => {
    const mock = await openDictionary(page);
    await page.getByLabel("New vocabulary term").fill("Tauri");
    await page.getByLabel("New vocabulary term").press("Enter");
    await expect(
      page.getByText("“Tauri” is already in the list."),
    ).toBeVisible();
    expect(
      mock.calls.filter((call) => call.cmd === "update_custom_words"),
    ).toHaveLength(0);
    await page.getByRole("button", { name: "Remove Tauri" }).click();
    await expect(
      page.getByRole("button", { name: "Remove Tauri" }),
    ).toHaveCount(0);
    await tabs(page)
      .getByRole("button", { name: /Crutches/ })
      .click();
    await page.getByLabel("New filler word or phrase").fill("UM");
    await page.getByLabel("New filler word or phrase").press("Enter");
    await expect(page.getByText("“um” is already in the list.")).toBeVisible();
  });

  test("reset failures report an error and retain the list", async ({
    page,
  }) => {
    await installTauriMock(page, {
      get_app_settings: SETTINGS,
      get_filler_words: ["um"],
      reset_filler_words: () => {
        throw new Error("reset unavailable");
      },
    });
    await page.goto("/");
    await page.getByTitle("Dictionary").click();
    await tabs(page)
      .getByRole("button", { name: /Crutches/ })
      .click();
    await page.getByRole("button", { name: "Restore defaults" }).click();
    await expect(
      page.getByText("Could not update filler words."),
    ).toBeVisible();
    await expect(page.getByRole("button", { name: "Remove um" })).toBeVisible();
  });
  test("a failed crutch save keeps the draft and reports the failure", async ({
    page,
  }) => {
    await installTauriMock(page, {
      get_app_settings: SETTINGS,
      get_filler_words: ["um"],
      set_filler_words: () => {
        throw new Error("save unavailable");
      },
    });
    await page.goto("/");
    await page.getByTitle("Dictionary").click();
    await tabs(page)
      .getByRole("button", { name: /Crutches/ })
      .click();
    await page.getByLabel("New filler word or phrase").fill("tipo");
    await page.getByRole("button", { name: "Add", exact: true }).click();
    await expect(page.getByLabel("New filler word or phrase")).toHaveValue(
      "tipo",
    );
    await expect(
      page.getByText("Could not update filler words."),
    ).toBeVisible();
  });

  test("pending writes disable edits and tab switching", async ({ page }) => {
    let release: (() => void) | undefined;
    const pending = new Promise<null>((resolve) => {
      release = () => resolve(null);
    });
    await installTauriMock(page, {
      get_app_settings: SETTINGS,
      get_filler_words: ["um"],
      set_filler_words: () => pending,
    });
    await page.goto("/");
    await page.getByTitle("Dictionary").click();
    await tabs(page)
      .getByRole("button", { name: /Crutches/ })
      .click();
    await page.getByLabel("New filler word or phrase").fill("tipo");
    await page.getByRole("button", { name: "Add", exact: true }).click();
    try {
      await expect(
        tabs(page).getByRole("button", { name: /Vocabulary/ }),
      ).toBeDisabled();
      await expect(page.getByLabel("New filler word or phrase")).toBeDisabled();
      await expect(
        page.getByRole("button", { name: "Remove um" }),
      ).toBeDisabled();
    } finally {
      release?.();
    }
    await expect(
      page.getByRole("button", { name: "Remove tipo" }),
    ).toBeVisible();
  });
  test("shows vocabulary with counts, search and per-term removal", async ({
    page,
  }) => {
    await openDictionary(page);
    await expect(
      tabs(page).getByRole("button", { name: /Vocabulary/ }),
    ).toHaveAttribute("aria-pressed", "true");
    await expect(
      tabs(page).getByRole("button", { name: /Vocabulary/ }),
    ).toContainText("2");
    await expect(
      page.getByText("Transcreve.ai", { exact: true }).first(),
    ).toBeVisible();

    await page.getByRole("searchbox").fill("tau");
    await expect(
      page.getByRole("button", { name: "Remove Tauri" }),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Remove Transcreve.ai" }),
    ).toHaveCount(0);
    await page.getByRole("searchbox").fill("nope");
    await expect(page.getByTestId("dictionary-empty")).toContainText(
      "Nothing matches",
    );
  });

  test("adds a vocabulary word through the settings store", async ({
    page,
  }) => {
    const mock = await openDictionary(page);
    await page.getByRole("button", { name: "Add word" }).click();
    await expect(page.getByLabel("New vocabulary term")).toBeFocused();
    await page.getByLabel("New vocabulary term").fill("whisper.cpp");
    await page.getByRole("button", { name: "Add", exact: true }).click();
    await expect(
      page.getByRole("button", { name: "Remove whisper.cpp" }),
    ).toBeVisible();
    expect(mock.calls.some((c) => c.cmd.includes("custom_words"))).toBe(true);
  });

  test("crutches tab edits the list and persists through set_filler_words", async ({
    page,
  }) => {
    const mock = await openDictionary(page);
    await tabs(page)
      .getByRole("button", { name: /Crutches/ })
      .click();
    await expect(page.getByRole("switch")).toBeChecked();
    await page.getByLabel("New filler word or phrase").fill("tipo");
    await page.getByRole("button", { name: "Add", exact: true }).click();
    await expect
      .poll(() => mock.calls.find((c) => c.cmd === "set_filler_words"))
      .toMatchObject({ args: { words: ["um", "like", "tipo"] } });
    await page.getByRole("button", { name: "Remove um" }).click();
    await expect
      .poll(() => mock.calls.filter((c) => c.cmd === "set_filler_words").length)
      .toBe(2);
    await page.getByRole("button", { name: "Restore defaults" }).click();
    await expect
      .poll(() => mock.calls.some((c) => c.cmd === "reset_filler_words"))
      .toBe(true);
  });

  test("replacements are shown as v1.1+ and cannot be added", async ({
    page,
  }) => {
    await openDictionary(page);
    const tab = tabs(page).getByRole("button", { name: /Replacements/ });
    await expect(tab).toContainText("v1.1+");
    await tab.click();
    await expect(page.getByTestId("dictionary-replacements")).toContainText(
      "Replacements are coming",
    );
    await expect(page.getByRole("button", { name: "Add word" })).toBeDisabled();
  });

  test("the preview removes the user's crutches and highlights vocabulary", async ({
    page,
  }) => {
    await openDictionary(page);
    const after = page.getByTestId("dictionary-after");
    await page
      .getByLabel("You said")
      .fill("send the file to Tauri, um, like, today");
    await expect(after).toContainText("Send the file to Tauri, today");
    await expect(after.locator("mark")).toHaveText("Tauri");

    // Turning the cleanup off keeps the crutches in the preview.
    await tabs(page)
      .getByRole("button", { name: /Crutches/ })
      .click();
    await page.getByRole("switch").uncheck();
    await expect(after).toContainText("um, like");
  });

  test("an empty vocabulary shows the empty state", async ({ page }) => {
    await installTauriMock(page, {
      get_app_settings: { ...SETTINGS, custom_words: [] },
      get_filler_words: [],
    });
    await page.goto("/");
    await page.getByTitle("Dictionary").click();
    await expect(page.getByTestId("dictionary-empty")).toContainText(
      "No words yet",
    );
  });
});
