import { test, expect, type Page } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";
import {
  startAssistantCoverage,
  stopAssistantCoverage,
} from "./helpers/assistant-coverage";
test.beforeEach(async ({ page }) => startAssistantCoverage(page));
test.afterEach(async ({ page }, info) => stopAssistantCoverage(page, info));
import { installTauriMock, emitTauriEvent } from "./helpers/tauri-mock";

const READY = {
  open: true,
  phase: "idle",
  dictating: false,
  providerId: "cli_agent/codex",
  providerLabel: "Codex",
  providerReady: true,
  providerHint: null,
  queuedPrompt: null,
  errorKind: null,
  errorDetail: null,
  messages: [],
  pinned: false,
};
async function open(
  page: Page,
  handlers: Parameters<typeof installTauriMock>[1] = {},
) {
  const mock = await installTauriMock(page, {
    assistant_get_state: READY,
    ...handlers,
  });
  await page.goto("/src/assistant/index.html");
  await expect(
    page.getByRole("textbox", { name: "Your message" }),
  ).toBeVisible();
  return mock;
}
const field = (page: Page) =>
  page.getByRole("textbox", { name: "Your message" });
const send = (page: Page) =>
  page.getByRole("button", { name: "Send message", exact: true });

test("assistant surface fills its window without an outer frame", async ({
  page,
}) => {
  await open(page);
  const panel = page.getByRole("dialog");
  await expect(panel).toHaveCSS("margin-top", "0px");
  await expect(panel).toHaveCSS("border-top-width", "0px");
  await expect(panel).toHaveCSS("box-shadow", "none");
});

test("assistant follows the application palette when the theme changes", async ({
  page,
}) => {
  await open(page);
  for (const theme of ["dark", "light"] as const) {
    await emitTauriEvent(page, "theme-changed", theme);
    await expect(page.locator("html")).toHaveAttribute("data-theme", theme);
    const matches = await page.locator(".as-stage").evaluate((element) => {
      const surface = getComputedStyle(element);
      const app = getComputedStyle(document.documentElement);
      return [
        ["--as-solid", "--panel"],
        ["--as-ink", "--text"],
        ["--as-accent", "--accent"],
      ].every(
        ([local, shared]) =>
          surface.getPropertyValue(local).trim() ===
          app.getPropertyValue(shared).trim(),
      );
    });
    expect(matches).toBe(true);
  }
});

test("typing sends trimmed text once and clears accepted draft", async ({
  page,
}) => {
  const mock = await open(page);
  await expect(send(page)).toBeDisabled();
  await field(page).fill("  A useful question  ");
  await field(page).press("Enter");
  await expect
    .poll(() => mock.calls.filter((c) => c.cmd === "assistant_send").length)
    .toBe(1);
  expect(mock.calls.find((c) => c.cmd === "assistant_send")?.args.text).toBe(
    "A useful question",
  );
  await expect(field(page)).toHaveValue("");
});
test("Shift Enter and IME do not send", async ({ page }) => {
  const mock = await open(page);
  await field(page).fill("First line");
  await field(page).press("Shift+Enter");
  await field(page).dispatchEvent("keydown", {
    key: "Enter",
    isComposing: true,
  });
  expect(mock.calls.filter((c) => c.cmd === "assistant_send")).toHaveLength(0);
  await expect(field(page)).toHaveValue("First line\n");
});
test("failed send retains draft and shows visible feedback", async ({
  page,
}) => {
  await open(page, {
    assistant_send: () => {
      throw { code: "Internal", message: "unavailable" };
    },
  });
  await field(page).fill("Keep my text");
  await send(page).click();
  await expect(page.getByRole("alert")).toContainText("Could not send");
  await expect(field(page)).toHaveValue("Keep my text");
  await expect(send(page)).toBeEnabled();
});
test("editing during accepted send does not lose newer text", async ({
  page,
}) => {
  let finish!: (value: null) => void;
  const pending = new Promise<null>((resolve) => {
    finish = resolve;
  });
  const mock = await open(page, { assistant_send: () => pending });
  await field(page).fill("Original");
  await send(page).click();
  await expect
    .poll(() => mock.calls.filter((c) => c.cmd === "assistant_send").length)
    .toBe(1);
  await expect(send(page)).toBeDisabled();
  await field(page).fill("New draft");
  finish(null);
  await expect(send(page)).toBeEnabled();
  await expect(field(page)).toHaveValue("New draft");
});
test("thinking, dictating and missing provider prevent typed sends", async ({
  page,
}) => {
  const mock = await open(page);
  await field(page).fill("Draft");
  for (const state of [
    { phase: "thinking" },
    { dictating: true },
    { providerReady: false, providerHint: "offline" },
  ]) {
    await emitTauriEvent(page, "assistant://state", { ...READY, ...state });
    await expect(send(page)).toBeDisabled();
    await field(page).press("Enter");
  }
  expect(mock.calls.filter((c) => c.cmd === "assistant_send")).toHaveLength(0);
  await expect(field(page)).toHaveValue("Draft");
});
test("new conversation clears draft and focuses input only after explicit action", async ({
  page,
}) => {
  const mock = await open(page);
  await expect(field(page)).not.toBeFocused();
  await field(page).fill("Old draft");
  await page.getByRole("button", { name: /new conversation/i }).click();
  await expect(field(page)).toHaveValue("");
  await expect(field(page)).toBeFocused();
  expect(mock.calls.some((c) => c.cmd === "assistant_new_conversation")).toBe(
    true,
  );
});
test("failed new conversation preserves draft", async ({ page }) => {
  await open(page, {
    assistant_new_conversation: () => {
      throw { code: "Internal", message: "failed" };
    },
  });
  await field(page).fill("Do not lose this");
  await page.getByRole("button", { name: /new conversation/i }).click();
  await expect(page.getByRole("alert")).toContainText("Could not start");
  await expect(field(page)).toHaveValue("Do not lose this");
});

test("new conversation acknowledgement preserves a newer draft", async ({
  page,
}) => {
  let finish!: (value: null) => void;
  const pending = new Promise<null>((resolve) => {
    finish = resolve;
  });
  const mock = await open(page, { assistant_new_conversation: () => pending });
  await field(page).fill("Old draft");
  await page.getByRole("button", { name: /new conversation/i }).click();
  await expect
    .poll(
      () =>
        mock.calls.filter((c) => c.cmd === "assistant_new_conversation").length,
    )
    .toBe(1);
  await field(page).fill("New conversation draft");
  finish(null);
  await expect(
    page.getByRole("button", { name: /new conversation/i }),
  ).toBeEnabled();
  await expect(field(page)).toHaveValue("New conversation draft");
});

test("pin snapshot cannot overwrite a newer response event", async ({
  page,
}) => {
  let finish!: (value: unknown) => void;
  let calls = 0;
  const pending = new Promise<unknown>((resolve) => {
    finish = resolve;
  });
  await open(page, {
    assistant_get_state: () => (++calls <= 2 ? READY : pending),
  });
  await page.getByRole("button", { name: "Dock to edge", exact: true }).click();
  await expect.poll(() => calls).toBe(3);
  await emitTauriEvent(page, "assistant://state", {
    ...READY,
    pinned: true,
    messages: [{ role: "assistant", content: "Newest answer" }],
  });
  finish({ ...READY, pinned: true });
  await expect(
    page.getByRole("button", { name: "Dock to edge", exact: true }),
  ).toBeEnabled();
  await expect(page.getByText("Newest answer", { exact: true })).toBeVisible();
});
test("pin is guarded until command completes and failure is visible", async ({
  page,
}) => {
  let reject!: (value: unknown) => void;
  const pending = new Promise<null>((_, r) => {
    reject = r;
  });
  const mock = await open(page, { assistant_set_panel_pinned: () => pending });
  const pin = page.getByRole("button", { name: "Dock to edge", exact: true });
  await pin.click();
  await expect(pin).toBeDisabled();
  expect(
    mock.calls.filter((c) => c.cmd === "assistant_set_panel_pinned"),
  ).toHaveLength(1);
  reject({ code: "Internal", message: "failed" });
  await expect(pin).toBeEnabled();
  await expect(pin).toHaveAttribute("aria-pressed", "false");
  await expect(page.getByRole("alert")).toContainText("Could not change");
});
test("voice button uses assistant route and Escape preserves typed draft", async ({
  page,
}) => {
  const mock = await open(page);
  await field(page).fill("Unsaved local draft");
  await page.getByRole("button", { name: "Dictate", exact: true }).click();
  await expect
    .poll(
      () =>
        mock.calls.filter((c) => c.cmd === "assistant_toggle_dictation").length,
    )
    .toBe(1);
  await field(page).press("Escape");
  await expect
    .poll(() => mock.calls.filter((c) => c.cmd === "assistant_dismiss").length)
    .toBe(1);
  await expect(field(page)).toHaveValue("Unsaved local draft");
});
for (const theme of ["light", "dark"]) {
  test(`glass assistant ${theme} accessible and respects reduced transparency`, async ({
    page,
  }) => {
    await page.setViewportSize({ width: 440, height: 640 });
    await page.addInitScript(
      (theme) => localStorage.setItem("transcreve-ai.theme", theme),
      theme,
    );
    await open(page, { get_app_settings: { theme } });
    await page.evaluate(
      (theme) => (document.documentElement.dataset.theme = theme),
      theme,
    );
    await expect(
      page.getByRole("button", { name: "Reduce transparency", exact: true }),
    ).toBeVisible();
    expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
    await page.screenshot({
      path: `docs/design/screens/assistant-implemented-${theme}.png`,
    });
    await page
      .getByRole("button", { name: "Reduce transparency", exact: true })
      .click();
    await expect(page.locator(".as-panel")).toHaveCSS(
      "backdrop-filter",
      "none",
    );
    expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
    await page.screenshot({
      path: `docs/design/screens/assistant-implemented-${theme}-solid.png`,
    });
  });
}
