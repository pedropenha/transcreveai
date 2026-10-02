/**
 * Overlay identity ("Papel & Anil", ADR-0003): the Flow Bar, the meeting toast
 * and the assistant panel are separate webviews. This checks they really
 * render with the self-hosted Instrument Sans and paint from the --ov-*
 * overlay tokens (dark in both themes), not from a stray hard-coded color.
 */

import { test, expect, type Page } from "@playwright/test";
import { emitTauriEvent, installTauriMock } from "./helpers/tauri-mock";

const SANS = '"Instrument Sans Variable"';

/** `rgb(r, g, b)` that a CSS color expression resolves to in `page`. */
async function resolveColor(page: Page, expression: string): Promise<string> {
  return page.evaluate((value) => {
    const probe = document.createElement("span");
    probe.style.color = value;
    document.body.appendChild(probe);
    const resolved = getComputedStyle(probe).color;
    probe.remove();
    return resolved;
  }, expression);
}

async function expectInstrumentSans(page: Page, selector: string) {
  // Idle surfaces draw no text, so load the face explicitly: this fails if
  // the self-hosted woff2 does not resolve from the webview's origin.
  const loaded = await page.evaluate(
    (font) => document.fonts.load(`14px ${font}`).then((f) => f.length),
    SANS,
  );
  expect(loaded).toBeGreaterThan(0);
  expect(
    await page.evaluate((font) => document.fonts.check(`14px ${font}`), SANS),
  ).toBe(true);
  const family = await page
    .locator(selector)
    .first()
    .evaluate((el) => getComputedStyle(el).fontFamily);
  expect(family).toContain("Instrument Sans Variable");
}

for (const scheme of ["light", "dark"] as const) {
  test.describe(`overlays render with the overlay tokens (${scheme})`, () => {
    test.beforeEach(async ({ page }) => {
      await page.emulateMedia({ colorScheme: scheme });
    });

    test("Flow Bar: Instrument Sans and no hard-coded hover surfaces", async ({
      page,
    }) => {
      await installTauriMock(page, {
        get_app_settings: {
          onboarding_completed: true,
          app_language: "en",
          flowbar_visibility: "always",
          overlay_style: "minimal",
        },
      });
      await page.goto("/src/overlay/index.html");
      await expectInstrumentSans(page, ".ov-stage");
      const hoverSurface = await page.evaluate(() =>
        getComputedStyle(document.documentElement)
          .getPropertyValue("--flowbar-hover-surface")
          .trim(),
      );
      // Resolves through var(--ov-bg), never a literal #0b0b0c.
      expect(hoverSurface.toLowerCase()).not.toBe("#0b0b0c");
      expect(await resolveColor(page, "var(--flowbar-hover-surface)")).toBe(
        await resolveColor(page, "var(--ov-bg)"),
      );
    });

    test("meeting toast: detection card paints from --ov-bg / --ov-cta", async ({
      page,
    }) => {
      const mock = await installTauriMock(page);
      await page.goto("/src/toast/index.html");
      await expect
        .poll(() =>
          mock.calls.some(
            (call) =>
              call.cmd === "plugin:event|listen" &&
              call.args.event === "toast://state",
          ),
        )
        .toBe(true);
      await emitTauriEvent(page, "toast://state", {
        collapsed: false,
        detection: {
          detection_id: "det-1",
          app_label: "Google Meet",
          exe: "chrome.exe",
          action: "ask",
          started_at: Date.now(),
        },
        notice: null,
      });
      const card = page.locator(".tcard.tmeeting");
      await expect(card).toBeVisible();
      await expectInstrumentSans(page, ".toast-stage");
      expect(
        await card.evaluate((el) => getComputedStyle(el).backgroundColor),
      ).toBe(await resolveColor(page, "var(--ov-bg)"));
      expect(await card.evaluate((el) => getComputedStyle(el).color)).toBe(
        await resolveColor(page, "var(--ov-text)"),
      );
      expect(await resolveColor(page, "var(--toast-cta-bg)")).toBe(
        await resolveColor(page, "var(--ov-cta)"),
      );
    });

    test("assistant panel: Instrument Sans on the overlay surface", async ({
      page,
    }) => {
      await installTauriMock(page, {
        assistant_get_state: {
          open: true,
          phase: "idle",
          dictating: false,
          providerId: null,
          providerLabel: null,
          providerReady: true,
          providerHint: null,
          queuedPrompt: null,
          errorKind: null,
          errorDetail: null,
          messages: [],
          pinned: false,
        },
      });
      await page.goto("/src/assistant/index.html");
      await expect(page.getByRole("dialog")).toBeVisible();
      await expectInstrumentSans(page, ".as-stage");
      const panel = page.locator(".as-panel");
      expect(
        await panel.evaluate((el) => getComputedStyle(el).borderTopColor),
      ).toBe(await resolveColor(page, "var(--ov-line)"));
    });
  });
}
