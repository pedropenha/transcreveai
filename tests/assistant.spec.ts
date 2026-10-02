/**
 * Assistant panel title-strip buttons (F012) — the /src/assistant webview
 * driven against the mocked Tauri IPC. Covers the reported failures:
 * close, pin and "new conversation" must each reach their command exactly
 * once per gesture, a fast second press must NOT be swallowed by dedup,
 * a press HELD past any time window must not double-fire, and a failed
 * close still hides the window (a dead-looking X is the bug).
 *
 * Native WebView2 hit-testing, first-click-on-unfocused and DPI scaling
 * still need the real-app smoke pass; here we prove the webview side
 * always delivers the gesture and handles the command's result.
 */

import { test, expect, type Page } from "@playwright/test";
import {
  installTauriMock,
  emitTauriEvent,
  type TauriMock,
} from "./helpers/tauri-mock";

const PANEL_URL = "/src/assistant/index.html";

const IDLE_STATE = {
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
};

async function openPanel(
  page: Page,
  handlers: Record<string, unknown> = {},
): Promise<TauriMock> {
  const mock = await installTauriMock(page, {
    // Specta commands wrap invoke() in the {status:"ok"} envelope
    // themselves — handlers return the raw payload.
    assistant_get_state: IDLE_STATE,
    ...handlers,
  });
  await page.goto(PANEL_URL);
  await expect(page.getByRole("dialog")).toBeVisible();
  return mock;
}

function countCalls(mock: TauriMock, cmd: string): number {
  return mock.calls.filter((call) => call.cmd === cmd).length;
}

const nextFrame = (page: Page) =>
  page.evaluate(() => new Promise((r) => requestAnimationFrame(() => r(null))));

test.describe("Assistant strip buttons — one action per gesture", () => {
  test("close, pin and new conversation each invoke their command once", async ({
    page,
  }) => {
    const mock = await openPanel(page);

    await page.getByRole("button", { name: /^close$/i }).click();
    // click() resolves after pointerdown+click both fired — the count is
    // already final, so a late duplicate cannot hide behind poll().
    expect(countCalls(mock, "assistant_close")).toBe(1);

    await page.getByRole("button", { name: /new conversation/i }).click();
    expect(countCalls(mock, "assistant_new_conversation")).toBe(1);

    await page.getByRole("button", { name: /^pin$/i }).click();
    expect(
      mock.calls.some(
        (call) =>
          call.cmd === "assistant_set_panel_pinned" &&
          call.args.pinned === true,
      ),
    ).toBe(true);
  });

  test("a fast second press counts — dedup only drops the gesture's own click echo", async ({
    page,
  }) => {
    const mock = await openPanel(page);
    const close = page.getByRole("button", { name: /^close$/i });

    // Two presses back-to-back: both are real gestures, both must run.
    await close.click();
    await close.click();
    expect(countCalls(mock, "assistant_close")).toBe(2);
  });

  test("a press held past the dedup window still fires exactly once", async ({
    page,
  }) => {
    const mock = await openPanel(page);
    const close = page.getByRole("button", { name: /^close$/i });
    const box = await close.boundingBox();
    expect(box).not.toBeNull();

    await page.mouse.move(box!.x + box!.width / 2, box!.y + box!.height / 2);
    await page.mouse.down();
    // Hold past 1 s: the click lands at release, so the dedup stamp must
    // be re-taken at pointerup — a stamp taken at pointerdown would look
    // stale here and let the click run a second time.
    await page.waitForTimeout(1200);
    await page.mouse.up();
    expect(countCalls(mock, "assistant_close")).toBe(1);
  });

  test("a pen/touch release fires once even though pointerout precedes click", async ({
    page,
  }) => {
    const mock = await openPanel(page);
    const close = page.getByRole("button", { name: /^close$/i });
    // Touch/pen order: pointerdown → pointerup → pointerout → click.
    // React synthesizes onPointerLeave from the native `pointerout` — a
    // native `pointerleave` dispatch would never reach the handler, so the
    // sequence must use pointerout with relatedTarget outside the button.
    // Clearing the dedup flag there would let the click run again — the
    // flag must survive non-mouse leaves.
    const seq: [string, Record<string, unknown>][] = [
      ["pointerdown", { pointerType: "pen", button: 0 }],
      ["pointerup", { pointerType: "pen", button: 0 }],
      ["pointerout", { pointerType: "pen", relatedTarget: null }],
      ["click", {}],
    ];
    for (const [type, init] of seq) {
      await close.dispatchEvent(type, {
        bubbles: true,
        cancelable: true,
        ...init,
      });
    }
    expect(countCalls(mock, "assistant_close")).toBe(1);
  });

  test("a pointercancel clears the stamp — a later click is a fresh gesture", async ({
    page,
  }) => {
    const mock = await openPanel(page);
    const close = page.getByRole("button", { name: /^close$/i });
    // pointerdown ran the action once; pointercancel voids the pairing —
    // a click that still arrives is its own gesture and must run.
    for (const [type, init] of [
      ["pointerdown", { button: 0 }],
      ["pointercancel", {}],
      ["click", {}],
    ] as [string, Record<string, unknown>][]) {
      await close.dispatchEvent(type, {
        bubbles: true,
        cancelable: true,
        ...init,
      });
    }
    expect(countCalls(mock, "assistant_close")).toBe(2);
  });

  test("keyboard activation works (Enter sends the command once)", async ({
    page,
  }) => {
    const mock = await openPanel(page);
    await page.getByRole("button", { name: /new conversation/i }).focus();
    await page.keyboard.press("Enter");
    await expect
      .poll(() => countCalls(mock, "assistant_new_conversation"))
      .toBe(1);
  });

  test("a state event during hydration wins over the stale snapshot", async ({
    page,
  }) => {
    // The snapshot resolves late — after a fresher assistant://state event
    // already rendered pinned state. The hydration apply must be skipped.
    // The snapshot carries a unique provider label so that applying it
    // would leave observable evidence ("STALE" in the title strip).
    let resolveHydration: (value: unknown) => void = () => {};
    const gate = new Promise<unknown>((resolve) => {
      resolveHydration = resolve;
    });
    const mock = await installTauriMock(page, {
      assistant_get_state: () => gate,
    });
    await page.goto(PANEL_URL);
    // Listeners register before hydration resolves.
    await expect
      .poll(() =>
        mock.calls.some(
          (call) =>
            call.cmd === "plugin:event|listen" &&
            call.args.event === "assistant://state",
        ),
      )
      .toBe(true);
    await emitTauriEvent(page, "assistant://state", {
      ...IDLE_STATE,
      pinned: true,
    });
    // Now the stale snapshot lands — unpinned, labeled STALE — and must
    // not win. The get_state invoke must actually have run, and we give
    // the apply a couple of frames to (not) happen.
    resolveHydration({ ...IDLE_STATE, providerLabel: "STALE" });
    // StrictMode double-mounts, so the snapshot fetch can run twice — the
    // guard must still let neither overwrite the event's pinned state.
    await expect
      .poll(() => countCalls(mock, "assistant_get_state"))
      .toBeGreaterThanOrEqual(1);
    await nextFrame(page);
    await nextFrame(page);
    await expect(page.getByRole("dialog")).toBeVisible();
    await expect(page.getByRole("button", { name: /^pin$/i })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    await expect(page.getByText("STALE")).not.toBeVisible();
  });

  test("a refused close command still hides the window", async ({ page }) => {
    const warnings: string[] = [];
    page.on("console", (msg) => {
      if (msg.type() === "warning") {
        warnings.push(msg.text());
      }
    });
    const mock = await openPanel(page, {
      // A backend refusal resolves the invoke with the {status:"error"}
      // envelope — the panel logs "refused" and still hides the window
      // instead of looking like a dead button.
      assistant_close: () => {
        throw { code: "internal", message: "simulated refusal" };
      },
    });
    await page.getByRole("button", { name: /^close$/i }).click();
    await expect.poll(() => countCalls(mock, "plugin:window|hide")).toBe(1);
    // The failure must be logged either as a refusal or a thrown error —
    // Playwright's exposed binding can re-wrap a non-Error throw, so the
    // exact channel is an implementation detail.
    expect(warnings.some((w) => w.includes("assistant_close"))).toBe(true);
  });

  test("a thrown IPC Error still hides the window", async ({ page }) => {
    const warnings: string[] = [];
    page.on("console", (msg) => {
      if (msg.type() === "warning") {
        warnings.push(msg.text());
      }
    });
    const mock = await openPanel(page, {
      // A real Error (e.g. a dead IPC channel) takes the rethrow branch of
      // the specta wrapper — the catch path, not the refusal branch.
      assistant_close: () => {
        throw new Error("ipc channel closed");
      },
    });
    await page.getByRole("button", { name: /^close$/i }).click();
    await expect.poll(() => countCalls(mock, "plugin:window|hide")).toBe(1);
    expect(warnings.some((w) => w.includes("assistant_close"))).toBe(true);
  });

  test("pinned panel ignores title-strip drags and updates aria-pressed", async ({
    page,
  }) => {
    const mock = await openPanel(page);

    const pin = page.getByRole("button", { name: /^pin$/i });
    await pin.click();
    expect(countCalls(mock, "assistant_set_panel_pinned")).toBe(1);

    // Positive control first: an unpinned drag DOES move the window — the
    // pinned assertion below then proves the gate, not a broken drag.
    const strip = page.locator(".as-title");
    const box = await strip.boundingBox();
    expect(box).not.toBeNull();
    const y = box!.y + box!.height / 2;
    await page.mouse.move(box!.x + 20, y);
    await page.mouse.down();
    await page.mouse.move(box!.x + 80, y);
    await nextFrame(page); // the move call is rAF-throttled
    await page.mouse.up();
    expect(countCalls(mock, "assistant_move_panel")).toBeGreaterThan(0);

    // Backend confirms pinned → strip shows the pressed state.
    const movesBefore = countCalls(mock, "assistant_move_panel");
    await emitTauriEvent(page, "assistant://state", {
      ...IDLE_STATE,
      pinned: true,
    });
    await expect(pin).toHaveAttribute("aria-pressed", "true");

    // A drag gesture on the pinned strip must not move the window — give
    // a would-be late move a beat to land before the negative assert.
    await page.mouse.move(box!.x + 20, y);
    await page.mouse.down();
    await page.mouse.move(box!.x + 80, y);
    await nextFrame(page);
    await page.mouse.up();
    await nextFrame(page);
    expect(countCalls(mock, "assistant_move_panel")).toBe(movesBefore);
  });
});
