/**
 * Meeting toast (F008 / T-062) geometry tests — the /src/toast webview driven
 * against the mocked Tauri IPC. Covers the window-height contract that
 * `toast_set_content_height` owns: the reported height must settle at the
 * card's FINAL layout size, not a mid-transition measurement — the expanded
 * actions and the ▾ menu live below the compact card and are only clickable
 * if the native window grew with them.
 *
 * WebView2 hit-testing and DPI scaling still need the native smoke pass; here
 * we prove the webview always reports the settled height and that every menu
 * row survives the card's overflow boundary.
 */

import { test, expect } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";
import {
  installTauriMock,
  emitTauriEvent,
  type TauriMock,
} from "./helpers/tauri-mock";

const TOAST_URL = "/src/toast/index.html";

const DETECTION = {
  collapsed: false,
  detection: {
    detection_id: "det-1",
    app_label: "Google Meet",
    exe: "chrome.exe",
    action: "ask",
    started_at: Date.now(),
  },
  notice: null,
};

async function openToast(
  page: import("@playwright/test").Page,
): Promise<TauriMock> {
  const mock = await installTauriMock(page);
  await page.goto(TOAST_URL);
  // The webview registered its toast://state listener before the test emits.
  await expect
    .poll(() =>
      mock.calls.some(
        (call) =>
          call.cmd === "plugin:event|listen" &&
          call.args.event === "toast://state",
      ),
    )
    .toBe(true);
  return mock;
}

/** Heights the webview reported to the native window, in order. */
function reportedHeights(mock: TauriMock): number[] {
  return mock.calls
    .filter((call) => call.cmd === "toast_set_content_height")
    .map((call) => Number(call.args.height))
    .filter((height) => Number.isFinite(height) && height > 0);
}

for (const scale of [1, 1.25, 1.5]) {
  test.describe(`Toast narrow-window regression @${scale}x`, () => {
    test.use({
      viewport: { width: 376, height: 480 },
      deviceScaleFactor: scale,
    });

    test("long app names ellipsize while the title and both controls stay whole", async ({
      page,
    }) => {
      await openToast(page);
      const app =
        "Reunião Semanal de Planejamento do Produto e Desenvolvimento";
      await emitTauriEvent(page, "toast://state", {
        ...DETECTION,
        detection: { ...DETECTION.detection, app_label: app },
      });
      await expect(page.locator(".tsub")).toHaveAttribute("title", app);
      await expect(page.locator(".tsub-text")).toHaveCSS(
        "text-overflow",
        "ellipsis",
      );
      const geometry = await page.locator(".tmeeting").evaluate((card) => {
        const title = card.querySelector<HTMLElement>(".ttitle")!;
        const subtitle = card.querySelector<HTMLElement>(".tsub-text")!;
        const row = card.querySelector<HTMLElement>(".trow")!;
        const buttons = [
          ...card.querySelectorAll<HTMLElement>(".tsplit button"),
        ];
        return {
          titleClipped: title.scrollWidth > title.clientWidth,
          subtitleClipped: subtitle.scrollWidth > subtitle.clientWidth,
          controlsContained: buttons.every(
            (button) =>
              button.getBoundingClientRect().right <=
              row.getBoundingClientRect().right,
          ),
        };
      });
      expect(geometry).toEqual({
        titleClipped: false,
        subtitleClipped: true,
        controlsContained: true,
      });
      await page.keyboard.press("Tab");
      await expect(
        page.getByRole("button", { name: /Start Notetaker/i }),
      ).toBeFocused();
      await page.keyboard.press("Tab");
      await expect(
        page.getByRole("button", { name: /More actions/i }),
      ).toBeFocused();
    });

    test("translation errors wrap completely and report the full card height", async ({
      page,
    }) => {
      const mock = await openToast(page);
      await emitTauriEvent(page, "toast://state", {
        collapsed: false,
        detection: null,
        notice: {
          kind: "translated_dictation_error",
          message: "translation_model_incompatible",
        },
      });
      await expect(page.locator(".tnotice-title")).toBeVisible();
      await expect(page.locator(".tnotice-body")).toContainText(/English/i);
      await page.locator(".tcard").evaluate(async (card) => {
        await Promise.all(
          card.getAnimations().map((animation) => animation.finished),
        );
      });
      const geometry = await page.locator(".tnotice").evaluate((card) => {
        const body = card.querySelector<HTMLElement>(".tnotice-body")!;
        const rect = card.getBoundingClientRect();
        const bodyRect = body.getBoundingClientRect();
        return {
          height: Math.ceil(rect.height) + 32,
          wrapped: bodyRect.height > 18,
          contained:
            bodyRect.right <= rect.right && bodyRect.bottom <= rect.bottom,
          titleFont: getComputedStyle(card.querySelector(".tnotice-title")!)
            .fontFamily,
        };
      });
      expect(geometry.wrapped).toBe(true);
      expect(geometry.contained).toBe(true);
      expect(geometry.titleFont).toContain("Instrument Sans");
      await expect
        .poll(() => reportedHeights(mock).at(-1))
        .toBe(geometry.height);
    });
  });
}

test("meeting detection and translation error have no axe accessibility violations", async ({
  page,
}) => {
  await openToast(page);
  for (const state of [
    DETECTION,
    {
      collapsed: false,
      detection: null,
      notice: {
        kind: "translated_dictation_error",
        message: "translation_model_incompatible",
      },
    },
  ]) {
    await emitTauriEvent(page, "toast://state", state);
    await expect(page.locator(".tcard")).toBeVisible();
    await page.locator(".tcard").evaluate(async (card) => {
      await Promise.all(
        card.getAnimations().map((animation) => animation.finished),
      );
    });
    const scan = await new AxeBuilder({ page })
      .include(".toast-stage")
      .analyze();
    expect(scan.violations).toEqual([]);
  }
});

test("Portuguese meeting title fits alongside the separate start and menu buttons", async ({
  page,
}) => {
  await page.setViewportSize({ width: 376, height: 480 });
  const mock = await installTauriMock(page, {
    get_app_settings: { app_language: "pt-BR" },
  });
  await page.goto(TOAST_URL);
  await expect
    .poll(() =>
      mock.calls.some(
        (call) =>
          call.cmd === "plugin:event|listen" &&
          call.args.event === "toast://state",
      ),
    )
    .toBe(true);
  await emitTauriEvent(page, "toast://state", DETECTION);
  await expect(page.locator(".ttitle")).toHaveText("Reunião detectada");
  expect(
    await page
      .locator(".ttitle")
      .evaluate((title) => title.scrollWidth <= title.clientWidth),
  ).toBe(true);
  await expect(
    page.getByRole("button", { name: "Iniciar Notetaker" }),
  ).toBeVisible();
  await expect(page.getByRole("button", { name: "Mais ações" })).toBeVisible();
});

test.describe("Meeting toast — window height tracks the settled card", () => {
  test("the detection card is always expanded and reports its settled height", async ({
    page,
  }) => {
    const mock = await openToast(page);
    await emitTauriEvent(page, "toast://state", DETECTION);

    const card = page.locator(".tcard");
    await expect(card).toBeVisible();
    // No hover needed: CTA + chevron are visible straight away.
    await expect(
      page.getByRole("button", { name: /Start Notetaker/i }),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: /More actions/i }),
    ).toBeVisible();
    // 72 card + 32 stage padding.
    await expect
      .poll(() => reportedHeights(mock).at(-1), { timeout: 3000 })
      .toBe(104);

    // The action row must be physically clickable, not just rendered.
    await page.getByRole("button", { name: /Start Notetaker/i }).click();
    await expect
      .poll(() =>
        mock.calls.some(
          (call) =>
            call.cmd === "detector_respond" && call.args.action === "start",
        ),
      )
      .toBe(true);
  });

  test("the menu grows the card and every row stays inside the reported height", async ({
    page,
  }) => {
    const mock = await openToast(page);
    await emitTauriEvent(page, "toast://state", DETECTION);

    const card = page.locator(".tcard");
    await expect
      .poll(() => reportedHeights(mock).at(-1), { timeout: 3000 })
      .toBe(104);

    await page.getByRole("button", { name: /More actions/i }).click();
    await expect(page.locator(".tmenu")).toBeVisible();

    // 72 row + 168 menu + 32 padding = 272 once the layout
    // settles — the menu rows must fit inside, not hang below the window.
    await expect
      .poll(() => reportedHeights(mock).at(-1), { timeout: 3000 })
      .toBe(272);

    // Hit-test the LAST row: it sits at the bottom edge of the grown card.
    const lastItem = page.getByRole("menuitem", {
      name: /Ignore this meeting/i,
    });
    await expect(lastItem).toBeVisible();
    const cardBox = await card.boundingBox();
    const itemBox = await lastItem.boundingBox();
    expect(cardBox).not.toBeNull();
    expect(itemBox).not.toBeNull();
    expect(itemBox!.y + itemBox!.height).toBeLessThanOrEqual(
      cardBox!.y + cardBox!.height + 0.5,
    );

    await lastItem.click();
    await expect
      .poll(() =>
        mock.calls.some(
          (call) =>
            call.cmd === "detector_respond" &&
            call.args.action === "ignore_meeting",
        ),
      )
      .toBe(true);
  });

  test("a notice with an action expands without hover and reports its grown height", async ({
    page,
  }) => {
    const mock = await openToast(page);
    await emitTauriEvent(page, "toast://state", {
      collapsed: false,
      detection: null,
      notice: {
        kind: "meeting_checkin",
        message: "Still in the meeting?",
        action: "checkin",
        meeting_id: "m-1",
      },
    });

    await expect(
      page.getByRole("button", { name: /^Continue$/i }),
    ).toBeVisible();
    // Actionable notices settle at ≥136 without any hover gesture.
    await expect
      .poll(() => reportedHeights(mock).at(-1), { timeout: 3000 })
      .toBeGreaterThanOrEqual(136);

    await page.getByRole("button", { name: /^Stop$/i }).click();
    await expect
      .poll(() =>
        mock.calls.some(
          (call) =>
            call.cmd === "meeting_checkin_respond" &&
            call.args.keepRecording === false,
        ),
      )
      .toBe(true);
  });

  test("the consent reminder's Copiar aviso copies the notice text and dismisses", async ({
    page,
  }) => {
    // FR-009-02: the post-start reminder carries `copy_consent` — the button
    // invokes meeting_consent_copy, puts the returned notice text on the
    // clipboard, then dismisses itself.
    const noticeText = "This meeting is being transcribed.";
    const mock = await installTauriMock(page, {
      meeting_consent_copy: noticeText,
    });
    await page.addInitScript(() => {
      const w = window as unknown as { __copied: string[] };
      w.__copied = [];
      Object.defineProperty(navigator, "clipboard", {
        configurable: true,
        value: {
          writeText: (text: string) => {
            w.__copied.push(text);
            return Promise.resolve();
          },
        },
      });
    });
    await page.goto(TOAST_URL);
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
      detection: null,
      notice: {
        kind: "meeting_consent",
        message: "Let participants know the meeting is being transcribed.",
        action: "copy_consent",
      },
    });

    const copyBtn = page.getByRole("button", { name: /Copy notice/i });
    await expect(copyBtn).toBeVisible();
    await copyBtn.click();

    await expect
      .poll(() =>
        mock.calls.some((call) => call.cmd === "meeting_consent_copy"),
      )
      .toBe(true);
    await expect
      .poll(() =>
        page.evaluate(() =>
          (window as unknown as { __copied: string[] }).__copied.at(-1),
        ),
      )
      .toBe(noticeText);
    // A copied notice counts as handled — it dismisses itself.
    await expect
      .poll(() => mock.calls.some((call) => call.cmd === "toast_dismiss"))
      .toBe(true);
  });
});

function respondCalls(mock: TauriMock): number {
  return mock.calls.filter((call) => call.cmd === "detector_respond").length;
}

test.describe("Meeting toast — app icon and dismiss", () => {
  test("renders the backend icon, the app label and a hover-only dismiss", async ({
    page,
  }) => {
    await openToast(page);
    const png =
      "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";
    await emitTauriEvent(page, "toast://state", {
      ...DETECTION,
      detection: { ...DETECTION.detection, icon: png },
    });
    await expect(page.locator(".tapp-icon")).toHaveAttribute("src", png);
    await expect(page.locator(".tsub")).toContainText("Google Meet");
    const dismiss = page.getByRole("button", { name: /dismiss/i });
    await expect(dismiss).toHaveCSS("opacity", "0");
    await page.locator(".tcard").hover();
    await expect(dismiss).toHaveCSS("opacity", "1");
  });
});

test.describe("Meeting toast — one action per gesture", () => {
  /** Open the toast expanded so the action row is hit-testable — and the
   *  card has settled at its final height (a mid-transition boundingBox
   *  can point at where the button used to be). */
  async function openExpanded(page: import("@playwright/test").Page) {
    const mock = await openToast(page);
    await emitTauriEvent(page, "toast://state", DETECTION);
    await expect(
      page.getByRole("button", { name: /Start Notetaker/i }),
    ).toBeVisible();
    await expect
      .poll(() => reportedHeights(mock).at(-1), { timeout: 3000 })
      .toBe(104);
    return mock;
  }

  test("one press invokes detector_respond exactly once", async ({ page }) => {
    const mock = await openExpanded(page);
    await page.getByRole("button", { name: /Start Notetaker/i }).click();
    // click() resolves after pointerdown+click both fired — the count is
    // already final, so a late duplicate cannot hide behind poll().
    expect(respondCalls(mock)).toBe(1);
  });

  test("a press held past the dedup window still fires once", async ({
    page,
  }) => {
    const mock = await installTauriMock(page);
    await page.goto(TOAST_URL);
    await expect
      .poll(() =>
        mock.calls.some(
          (call) =>
            call.cmd === "plugin:event|listen" &&
            call.args.event === "toast://state",
        ),
      )
      .toBe(true);
    // Every meeting-card button collapses or unmounts on press (Start
    // flips to "Recording"; ✕ and the ▾ trigger call dismissTransient
    // which drops `expanded`). A notice action runs runNoticeAction
    // instead — the card and the button stay put, so the release-click
    // genuinely pairs with the press.
    await emitTauriEvent(page, "toast://state", {
      collapsed: false,
      detection: null,
      notice: {
        kind: "meeting_checkin",
        message: "Still in the meeting?",
        action: "checkin",
        meeting_id: "m-1",
      },
    });
    const keepGoing = page.getByRole("button", { name: /^Continue$/i });
    await expect(keepGoing).toBeVisible();
    const box = await keepGoing.boundingBox();
    expect(box).not.toBeNull();
    await page.mouse.move(box!.x + box!.width / 2, box!.y + box!.height / 2);
    await page.mouse.down();
    // Positive control: the button must survive the press — a release on
    // a detached element pairs with no click and proves nothing.
    await expect(keepGoing).toBeVisible();
    // The click lands at release — freshness must measure from pointerup,
    // not the press, or a >1 s hold would double-fire.
    await page.waitForTimeout(1200);
    await page.mouse.up();
    await page.waitForTimeout(150);
    expect(
      mock.calls.filter((call) => call.cmd === "meeting_checkin_respond")
        .length,
    ).toBe(1);
  });

  test("a pen/touch release fires once even though pointerout precedes click", async ({
    page,
  }) => {
    const mock = await openExpanded(page);
    // Touch/pen order: pointerdown → pointerup → pointerout → click.
    // React synthesizes onPointerLeave from the native `pointerout` — a
    // native `pointerleave` dispatch never reaches the handler, so this
    // must use pointerout with relatedTarget outside the button. Clearing
    // the dedup flag there would let the click rerun.
    // Dispatched in one synchronous evaluate: a real press replaces the
    // card with the "Recording" face on pointerdown, so per-event awaits
    // would find the button detached.
    const stillConnected = await page.evaluate(() => {
      const el = document.querySelector(".tsplit-main");
      if (!el) throw new Error("action button not rendered");
      const init = { bubbles: true, cancelable: true };
      el.dispatchEvent(
        new PointerEvent("pointerdown", { ...init, pointerType: "pen" }),
      );
      // Positive control: if React flushed between dispatches the element
      // is detached and the later events never reached it — the count
      // assertion below would be vacuous, so pin the wiring first.
      const connectedAfterDown = el.isConnected;
      el.dispatchEvent(
        new PointerEvent("pointerup", { ...init, pointerType: "pen" }),
      );
      el.dispatchEvent(
        new PointerEvent("pointerout", {
          ...init,
          pointerType: "pen",
          relatedTarget: null,
        }),
      );
      el.dispatchEvent(new MouseEvent("click", init));
      return connectedAfterDown;
    });
    expect(stillConnected).toBe(true);
    // Settle, then the final count — a late duplicate must not slip in.
    await page.waitForTimeout(150);
    expect(respondCalls(mock)).toBe(1);
  });

  test("Start shows the recording face and the toast dismisses itself", async ({
    page,
  }) => {
    const mock = await openExpanded(page);
    await page.getByRole("button", { name: /Start Notetaker/i }).click();
    // The post-await path is guarded by mountedRef — under StrictMode's
    // double-mount a ref cleared without re-arming would swallow this
    // whole face: no "Recording" text, no 3 s auto-dismiss.
    await expect(page.locator(".tconfirm .ttitle")).toContainText(
      /Recording|Gravando/i,
    );
    await expect
      .poll(() => mock.calls.some((call) => call.cmd === "toast_dismiss"), {
        timeout: 5000,
      })
      .toBe(true);
  });

  test("menu arrows work when focus stayed on the trigger", async ({
    page,
  }) => {
    await openExpanded(page);
    const trigger = page.getByRole("button", { name: /More actions/i });
    await trigger.click();
    await expect(page.locator(".tmenu")).toBeVisible();
    // A mouse-opened menu on an unfocused webview can leave focus on the
    // trigger; ArrowUp there must enter the menu at its LAST item.
    await trigger.focus();
    await page.keyboard.press("ArrowUp");
    await expect(page.getByRole("menuitem").last()).toBeFocused();
    // ArrowDown from the trigger enters at the FIRST item.
    await trigger.focus();
    await page.keyboard.press("ArrowDown");
    await expect(page.getByRole("menuitem").first()).toBeFocused();
    // And stepping back up from the first item wraps to the last.
    await page.keyboard.press("ArrowUp");
    await expect(page.getByRole("menuitem").last()).toBeFocused();
  });

  test("Escape closes the menu and returns focus to the trigger", async ({
    page,
  }) => {
    await openExpanded(page);
    const trigger = page.getByRole("button", { name: /More actions/i });
    // Open via keyboard: a mouse click's default button-focus races the
    // open-effect's item focus. Enter drives the click path directly.
    await trigger.focus();
    await page.keyboard.press("Enter");
    await expect(page.locator(".tmenu")).toBeVisible();
    // Opening moves focus to the first item — Escape from there must
    // close the menu and return focus to the trigger (menu pattern).
    await expect(page.getByRole("menuitem").first()).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(page.locator(".tmenu")).not.toBeVisible();
    await expect(trigger).toBeFocused();
  });
});

test.describe("Meeting toast — height reporting resilience", () => {
  test("a rejected height report retries until it lands", async ({ page }) => {
    let failuresLeft = 2;
    let heightCalls = 0;
    const mock = await installTauriMock(page, {
      toast_set_content_height: () => {
        heightCalls += 1;
        if (failuresLeft-- > 0) {
          throw new Error("simulated ipc failure");
        }
        return null;
      },
    });
    await page.goto(TOAST_URL);
    // Wait for the listener before emitting — an early toast://state is
    // simply missed.
    await expect
      .poll(() =>
        mock.calls.some(
          (call) =>
            call.cmd === "plugin:event|listen" &&
            call.args.event === "toast://state",
        ),
      )
      .toBe(true);
    await emitTauriEvent(page, "toast://state", DETECTION);
    await expect(page.locator(".tcard")).toBeVisible();
    // Two failures then success: only a real retry chain reaches ≥3
    // invocations — counting the last reported value alone would pass on
    // the very first (rejected) call. (A ResizeObserver callback can add
    // an interleaved call during the failure window, so the exact count
    // is not pinned.)
    await expect
      .poll(() => heightCalls, { timeout: 3000 })
      .toBeGreaterThanOrEqual(3);
    // The retry is bounded: once the report lands it must stop, not loop.
    const settled = heightCalls;
    await page.waitForTimeout(400);
    expect(heightCalls).toBe(settled);
    await expect
      .poll(() => reportedHeights(mock).at(-1), { timeout: 3000 })
      .toBe(104);
  });
});
