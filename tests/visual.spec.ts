import { test } from "@playwright/test";
import { installTauriMock } from "./helpers/tauri-mock";

/**
 * Visual regression captures for the Hub ("Papel & Anil" proposal).
 * Opt-in (CAPTURE_SCREENS=1) because it writes PNGs into docs/design/screens/
 * for human comparison against docs/design/proposta-ui.html.
 */
const WIDTHS = [900, 1280, 1600] as const;
const SCHEMES = ["light", "dark"] as const;

test.describe("hub screenshots", () => {
  test.skip(!process.env.CAPTURE_SCREENS, "set CAPTURE_SCREENS=1 to capture");

  for (const scheme of SCHEMES) {
    for (const width of WIDTHS) {
      test(`hub home ${scheme} ${width}px`, async ({ page }) => {
        await installTauriMock(page);
        await page.emulateMedia({ colorScheme: scheme });
        await page.setViewportSize({ width, height: 800 });
        await page.goto("/");
        await page.waitForSelector(".hub-rail, [aria-label='Main navigation']");
        await page.waitForTimeout(400);
        await page.screenshot({
          path: `docs/design/screens/${process.env.SCREEN_PREFIX ?? "hub"}-${scheme}-${width}.png`,
        });
      });
    }
  }
});
