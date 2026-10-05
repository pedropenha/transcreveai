/**
 * Visual evidence for the meeting toast layout (docs/design/screens/toast-*).
 *
 * Opt-in: `TOAST_SCREENSHOTS=1 bunx playwright test tests/toast-screens.spec.ts`
 * (`TOAST_SHOT_TAG` picks the file prefix, default "after"). The window is
 * rendered at its real logical width (376 px) on a mid-grey "desktop" so the
 * shadow margin is judged against the same transparent-window clipping the
 * native toast has. Device scale factors mimic Windows 100 % / 125 % / 150 %.
 */

import { test, expect } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import {
  installTauriMock,
  emitTauriEvent,
  type TauriMock,
} from "./helpers/tauri-mock";

const ENABLED = process.env.TOAST_SCREENSHOTS === "1";
const TAG = process.env.TOAST_SHOT_TAG ?? "after";
const OUT_DIR = join(process.cwd(), "docs", "design", "screens");
const TOAST_URL = "/src/toast/index.html";
const WINDOW_WIDTH = 376;

const EXE_ICON =
  "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

function detection(app: string, extra: Record<string, unknown> = {}) {
  return {
    collapsed: false,
    detection: {
      detection_id: `det-${app}`,
      app_label: app,
      exe: "app.exe",
      action: "ask",
      ...extra,
    },
    notice: null,
  };
}

function notice(kind: string, message: string, action?: string) {
  return {
    collapsed: false,
    detection: null,
    notice: { kind, message, action: action ?? null },
  };
}

interface Shot {
  name: string;
  state: unknown;
  /** Window width override (default 376, the real native window). */
  width?: number;
  /** Extra interaction before the capture. */
  prepare?: (page: import("@playwright/test").Page) => Promise<void>;
}

const SHOTS: Shot[] = [
  { name: "detect-meet", state: detection("Google Meet") },
  { name: "detect-zoom", state: detection("Zoom") },
  { name: "detect-teams", state: detection("Microsoft Teams") },
  {
    name: "detect-unknown-exe",
    state: detection("Reunião Semanal de Planejamento do Produto", {
      icon: EXE_ICON,
    }),
  },
  { name: "detect-meet-wide", width: 456, state: detection("Google Meet") },
  {
    name: "detect-unknown-wide",
    width: 456,
    state: detection("Reunião Semanal de Planejamento do Produto", {
      icon: EXE_ICON,
    }),
  },
  {
    name: "detect-menu",
    state: detection("Google Meet"),
    prepare: async (page) => {
      await page
        .getByRole("button", { name: /More actions|Mais ações/i })
        .click();
      await page.locator(".tmenu").waitFor();
    },
  },
  {
    name: "detect-focus-chevron",
    state: detection("Google Meet"),
    prepare: async (page) => {
      await page.keyboard.press("Tab");
      await page.keyboard.press("Tab");
    },
  },
  {
    name: "recording",
    state: detection("Google Meet"),
    prepare: async (page) => {
      await page
        .getByRole("button", { name: /Start Notetaker|Iniciar Notetaker/i })
        .click();
      await page.locator(".tconfirm").waitFor();
    },
  },
  {
    name: "notice-short",
    state: notice("info", "Reunião encerrada. Resumo em andamento."),
  },
  {
    name: "notice-translation-error",
    state: notice(
      "translated_dictation_error",
      "translation_model_incompatible",
    ),
  },
  {
    name: "notice-action",
    state: notice(
      "meeting_checkin",
      "Ainda em reunião? A gravação continua até você responder.",
      "checkin",
    ),
  },
];

const SCALES = [1, 1.5];
const SCHEMES = ["dark", "light"] as const;

async function settledHeight(mock: TauriMock): Promise<number> {
  let last = 0;
  let stable = 0;
  for (let i = 0; i < 40 && stable < 4; i += 1) {
    await new Promise((r) => setTimeout(r, 100));
    const heights = mock.calls
      .filter((c) => c.cmd === "toast_set_content_height")
      .map((c) => Number(c.args.height));
    const now = heights.at(-1) ?? 0;
    stable = now === last && now > 0 ? stable + 1 : 0;
    last = now;
  }
  return last;
}

for (const scale of SCALES) {
  for (const scheme of SCHEMES) {
    test.describe(`toast screenshots @${scale}x ${scheme}`, () => {
      test.skip(!ENABLED, "set TOAST_SCREENSHOTS=1 to capture");
      test.use({
        deviceScaleFactor: scale,
        colorScheme: scheme,
        viewport: { width: WINDOW_WIDTH, height: 400 },
      });

      for (const shot of SHOTS) {
        test(`${shot.name}`, async ({ page }) => {
          mkdirSync(OUT_DIR, { recursive: true });
          const mock = await installTauriMock(page);
          await page.addInitScript((s) => {
            try {
              localStorage.setItem("theme", s);
            } catch {
              /* storage unavailable */
            }
          }, scheme);
          await page.setViewportSize({
            width: shot.width ?? WINDOW_WIDTH,
            height: 400,
          });
          await page.goto(TOAST_URL);
          await page.evaluate((s) => {
            document.documentElement.dataset.theme = s;
          }, scheme);
          await page.addStyleTag({
            content:
              "html,body{background:linear-gradient(135deg,#8b93a1,#b7bcc6)!important}",
          });
          await emitTauriEvent(page, "toast://state", shot.state);
          await page.locator(".tcard").waitFor();
          if (shot.prepare) await shot.prepare(page);
          const height = await settledHeight(mock);
          const width = shot.width ?? WINDOW_WIDTH;
          await page.setViewportSize({
            width,
            height: Math.max(height, 88),
          });
          // Resizing moves the card away from the cursor: re-hover it so the
          // menu (which closes on mouseleave) stays open for the capture.
          const box = await page.locator(".tcard").boundingBox();
          if (box) {
            await page.mouse.move(
              box.x + box.width / 2,
              box.y + box.height / 2,
            );
          }
          // Resizing can deliver mouseleave before the move and close the
          // menu. Reopen at final geometry so this evidence has all actions.
          if (shot.name === "detect-menu") {
            if (!(await page.locator(".tmenu").isVisible())) {
              await page
                .getByRole("button", { name: /More actions|Mais ações/i })
                .click();
            }
            await expect(page.getByRole("menuitem")).toHaveCount(4);
            await expect(page.locator(".tmenu")).toBeVisible();
          }
          await page.waitForTimeout(250);
          await page.screenshot({
            path: join(
              OUT_DIR,
              `toast-${TAG}-${shot.name}-${scheme}-${Math.round(scale * 100)}.png`,
            ),
          });
        });
      }
    });
  }
}
