/**
 * Flow Bar (F001 / T-040) UI tests — the overlay entry at
 * /src/overlay/index.html driven against the mocked Tauri IPC. Covers the
 * AC-001-01..03 / AC-001-07..08 slice that lives in the webview: the idle
 * slit, the two-action hover card with its shortcut tooltip, and the
 * session-state faces (recording levels, done flash, error + retry).
 *
 * Native click-through (NFR-001-02) can't be exercised from a browser — the
 * spec only verifies the webview keeps reporting its interactive bounds via
 * `flowbar_set_hover`; the cursor hit-test loop is covered by Rust unit
 * tests in src-tauri/src/overlay/.
 */

import { test, expect } from "@playwright/test";
import {
  installTauriMock,
  emitTauriEvent,
  DEFAULT_APP_SETTINGS,
  type TauriMock,
} from "./helpers/tauri-mock";

const OVERLAY_URL = "/src/overlay/index.html";

const SESSION = {
  session_id: "test-session",
  mode: "push_to_talk",
  pending: 0,
};

/** Settings for an always-on Flow Bar docked bottom-center (the default). */
const ALWAYS_ON_SETTINGS = {
  ...DEFAULT_APP_SETTINGS,
  overlay_style: "minimal",
  overlay_position: "bottom",
  flowbar_visibility: "always",
  flowbar_position_edge: "bottom",
  flowbar_position_offset: 0.5,
  bindings: {
    transcribe: {
      id: "transcribe",
      name: "Transcribe",
      description: "",
      default_binding: "ctrl+win",
      current_binding: "ctrl+win",
    },
    assistant: {
      id: "assistant",
      name: "Assistant",
      description: "",
      default_binding: "ctrl+shift+a",
      current_binding: "ctrl+shift+a",
    },
  },
};

async function openFlowbar(
  page: import("@playwright/test").Page,
  settings = ALWAYS_ON_SETTINGS,
): Promise<TauriMock> {
  const mock = await installTauriMock(page, { get_app_settings: settings });
  await page.goto(OVERLAY_URL);
  await expect
    .poll(() =>
      mock.calls.some(
        (call) =>
          call.cmd === "plugin:event|listen" &&
          call.args.event === "meeting://state",
      ),
    )
    .toBe(true);
  return mock;
}

const sessionEvent = (state: string, extra: Record<string, unknown> = {}) => ({
  ...SESSION,
  state,
  ...extra,
});

test.describe("Flow Bar — idle slit & hover actions", () => {
  test("always-on mode shows the idle slit on load", async ({ page }) => {
    const mock = await openFlowbar(page);
    await expect(page.locator(".f-idle")).toBeVisible();
    // The webview reported its interactive bounds so the core keeps the rest
    // of the frame click-through (NFR-001-02 contract).
    await expect
      .poll(() => mock.calls.some((c) => c.cmd === "flowbar_set_hover"))
      .toBe(true);
  });

  test("hover reveals the three actions with the configured shortcut tooltip", async ({
    page,
  }) => {
    const mock = await openFlowbar(page);
    const slit = page.locator(".f-idle");
    await expect(slit).toBeVisible();

    await slit.hover();
    // FR-001-02: 120 ms enter delay before the card opens.
    const dictate = page.getByRole("button", { name: /Dictate/i });
    const notetaker = page.getByRole("button", { name: /Notetaker/i });
    const assistant = page.getByRole("button", { name: /Assistant/i });
    await expect(dictate).toBeVisible();
    await expect(notetaker).toBeVisible();
    await expect(assistant).toBeVisible();
    await expect(page.getByRole("button")).toHaveCount(3);

    // FR-001-03: the tooltip shows the configured shortcut, not a hardcoded one.
    await dictate.hover();
    await expect(page.locator(".fbar-tip")).toContainText("Dictate");
    await expect(page.locator(".fbar-tip")).toContainText("Ctrl + Win");

    // FR-001-03/06: clicking Ditar toggles a hands-free session through the
    // same edge as the configured shortcut.
    await dictate.click();
    expect(mock.calls.map((c) => c.cmd)).toContain("flowbar_toggle_dictation");

    // FR-001-04: the second action is the Notetaker start (T-064 seam).
    // The hover card is still open after the click — re-assert it is.
    await expect(notetaker).toBeVisible();
    await notetaker.click();
    expect(mock.calls.map((c) => c.cmd)).toContain("flowbar_start_notetaker");

    // FR-012-10: the third action opens the assistant overlay; its tooltip
    // advertises the configured `assistant` binding.
    await assistant.hover();
    await expect(page.locator(".fbar-tip")).toContainText("Assistant");
    await expect(page.locator(".fbar-tip")).toContainText("Ctrl + Shift + A");
    await assistant.click();
    expect(mock.calls.map((c) => c.cmd)).toContain("flowbar_open_assistant");
  });

  test("session-only mode renders nothing until a session shows", async ({
    page,
  }) => {
    await openFlowbar(page, {
      ...ALWAYS_ON_SETTINGS,
      flowbar_visibility: "during_recording",
    });
    await expect(page.locator(".f-idle")).toHaveCount(0);
    // The live region stays mounted while the visual card is hidden, so the
    // first state change is announced instead of mounting already populated.
    await expect(page.locator(".fbar-sr[aria-live='polite']")).toHaveCount(1);

    await emitTauriEvent(page, "show-overlay", "recording");
    await emitTauriEvent(page, "session://state", sessionEvent("recording"));
    await expect(page.locator(".f-rec")).toBeVisible();
  });
});

test.describe("Flow Bar — session states", () => {
  test("recording shows cancel, waveform and stop; levels drive the bars (AC-001-07)", async ({
    page,
  }) => {
    const mock = await openFlowbar(page);
    await emitTauriEvent(page, "show-overlay", "recording");
    await emitTauriEvent(page, "session://state", sessionEvent("recording"));

    await expect(page.locator(".f-rec")).toBeVisible();
    await expect(page.getByRole("button", { name: /Cancel/i })).toBeVisible();
    await expect(page.getByRole("button", { name: /Stop/i })).toBeVisible();

    // The waveform only reacts once the backend reports real samples flowing.
    await emitTauriEvent(page, "recording-ready", null);

    // Muted/no-input audio and quiet room tone → flat bars.
    const heightOf = (i: number) =>
      page
        .locator(".swave i")
        .nth(i)
        .evaluate((el) => el.clientHeight);
    const flat = await heightOf(3);
    for (let i = 0; i < 12; i += 1) {
      await emitTauriEvent(page, "audio://level", {
        rms: Array(16).fill(0.05),
      });
    }
    // Let the smoothed React state settle before reading the rendered height.
    await page.waitForTimeout(100);
    expect(await heightOf(3)).toBe(flat);

    await emitTauriEvent(page, "audio://level", {
      rms: Array(16).fill(0.95),
    });
    await expect.poll(() => heightOf(3)).toBeGreaterThan(flat);

    // The ■ button toggles the session off through the same binding edge.
    await page.getByRole("button", { name: /Stop/i }).click();
    expect(mock.calls.map((c) => c.cmd)).toContain("flowbar_toggle_dictation");
  });

  test("live streaming text updates while the session is recording", async ({
    page,
  }) => {
    const mock = await openFlowbar(page);
    await emitTauriEvent(page, "show-overlay", "streaming");
    await emitTauriEvent(page, "session://state", sessionEvent("recording"));

    const reportedRect = () =>
      mock.calls
        .filter((call) => call.cmd === "flowbar_set_hover")
        .map((call) => call.args.rect)
        .filter(
          (
            rect,
          ): rect is { x: number; y: number; width: number; height: number } =>
            rect !== null,
        )
        .at(-1);
    await expect.poll(() => reportedRect()?.width).toBeGreaterThan(0);
    const collapsedWidth = reportedRect()?.width ?? 0;

    await emitTauriEvent(page, "stream-text-event", {
      committed: "hello",
      tentative: " world",
    });
    // The live panel grows as text arrives; the native hit-test bounds must
    // grow with it or the transcript area becomes click-through mid-session.
    await expect
      .poll(() => reportedRect()?.width ?? 0)
      .toBeGreaterThan(collapsedWidth);
    await expect(page.locator(".stext .committed")).toContainText("hello");
    await expect(page.locator(".stext .tentative")).toContainText("world");

    await emitTauriEvent(page, "stream-phase-event", {
      phase: "working",
      kind: "transcribing",
    });
    await expect(page.locator(".sspinner")).toBeVisible();
    await expect(page.locator(".stext .committed")).toContainText("hello");
  });

  test("meeting recording shows live audio bars alongside its controls", async ({
    page,
  }) => {
    await openFlowbar(page);
    await emitTauriEvent(page, "meeting://state", {
      meeting_id: "meeting-1",
      status: "recording",
      elapsed_ms: 4_000,
    });

    const meeting = page.locator(".f-meeting");
    await expect(meeting).toBeVisible();
    await expect(page.getByRole("button", { name: /Pause/i })).toBeVisible();
    await expect(
      page.getByRole("button", { name: /Stop meeting/i }),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: /elapsed time 0:04/i }),
    ).toBeVisible();
    await expect(meeting.locator(".swave i")).toHaveCount(7);

    const heightOf = (i: number) =>
      meeting
        .locator(".swave i")
        .nth(i)
        .evaluate((element) => element.clientHeight);
    const flat = await heightOf(3);
    await emitTauriEvent(page, "audio://level", { rms: Array(16).fill(0.95) });
    await expect.poll(() => heightOf(3)).toBeGreaterThan(flat);

    await emitTauriEvent(page, "meeting://state", {
      meeting_id: "meeting-1",
      status: "paused",
      elapsed_ms: 5_000,
    });
    await expect(page.getByRole("button", { name: /Resume/i })).toBeVisible();
    await expect.poll(() => heightOf(3)).toBe(3);
  });

  test("paused meeting is restored from the backend snapshot on mount", async ({
    page,
  }) => {
    await installTauriMock(page, {
      get_app_settings: ALWAYS_ON_SETTINGS,
      meeting_current: {
        meeting_id: "meeting-paused",
        status: "paused",
        elapsed_ms: 42_000,
      },
    });
    await page.goto(OVERLAY_URL);

    await expect(page.locator(".f-meeting")).toBeVisible();
    await expect(page.getByRole("button", { name: /Resume/i })).toBeVisible();
    await expect(
      page.getByRole("button", { name: /elapsed time 0:42/i }),
    ).toBeVisible();
  });

  test("meeting processing and terminal states stay visible and reopen the transcript", async ({
    page,
  }) => {
    const mock = await openFlowbar(page);
    await emitTauriEvent(page, "meeting://state", {
      meeting_id: "meeting-1",
      status: "recording",
      elapsed_ms: 4_000,
    });
    await expect(page.locator(".f-meeting")).toBeVisible();

    // A delayed terminal event from an older meeting cannot steal the live
    // stop control; only the active meeting owns this pill.
    await emitTauriEvent(page, "meeting://state", {
      meeting_id: "meeting-old",
      status: "ready",
      elapsed_ms: 4_000,
    });
    await expect(page.locator(".f-meeting")).toBeVisible();
    await expect(page.getByRole("button", { name: /Pause/i })).toBeVisible();

    await emitTauriEvent(page, "meeting://state", {
      meeting_id: "meeting-1",
      status: "processing",
      elapsed_ms: 4_000,
    });
    const working = page.getByRole("button", {
      name: /transcribing meeting/i,
    });
    await expect(working).toBeVisible();
    await working.click();
    expect(
      mock.calls.find(
        (call) =>
          call.cmd === "meeting_window_open" &&
          call.args.meetingId === "meeting-1",
      ),
    ).toBeTruthy();

    await emitTauriEvent(page, "meeting://state", {
      meeting_id: "meeting-1",
      status: "ready",
      elapsed_ms: 4_000,
    });
    const ready = page.getByRole("button", {
      name: /open meeting transcript/i,
    });
    await expect(ready).toBeVisible();
    await ready.click();
    expect(
      mock.calls.find(
        (call) =>
          call.cmd === "meeting_window_open" &&
          call.args.meetingId === "meeting-1",
      ),
    ).toBeTruthy();

    await emitTauriEvent(page, "meeting://state", {
      meeting_id: "meeting-2",
      status: "error",
      elapsed_ms: 8_000,
    });
    const failed = page.getByRole("button", {
      name: /meeting processing failed/i,
    });
    await expect(failed).toBeVisible();
    await failed.click();
    expect(
      mock.calls.find(
        (call) =>
          call.cmd === "meeting_window_open" &&
          call.args.meetingId === "meeting-2",
      ),
    ).toBeTruthy();
  });

  test("done and error faces come from session://state; error hover offers retry (AC-001-08)", async ({
    page,
  }) => {
    const mock = await openFlowbar(page);
    await emitTauriEvent(page, "show-overlay", "recording");
    await emitTauriEvent(page, "session://state", sessionEvent("recording"));
    await emitTauriEvent(page, "session://state", sessionEvent("done"));
    await expect(page.locator(".f-done")).toBeVisible();

    await emitTauriEvent(
      page,
      "session://state",
      sessionEvent("error", { error: "model unavailable" }),
    );
    const errorPill = page.locator(".f-error");
    await expect(errorPill).toBeVisible();

    // Hovering the error pill reveals the concise cause + "Try again".
    await errorPill.hover();
    await expect(page.locator(".fbar-tip")).toContainText("model unavailable");
    const retry = page.getByRole("button", { name: /Try again/i });
    await retry.click();
    expect(mock.calls.map((c) => c.cmd)).toContain("flowbar_retry_last_failed");
  });

  test("nothing-heard flash and idle fallback after session://state idle", async ({
    page,
  }) => {
    await openFlowbar(page);
    await emitTauriEvent(page, "show-overlay", "nothing-heard");
    await emitTauriEvent(
      page,
      "session://state",
      sessionEvent("idle", { notice: "nothing_heard" }),
    );
    await expect(page.locator(".f-heard")).toContainText("Nothing heard");

    await emitTauriEvent(page, "hide-overlay", null);
    // The hide event ends the one-shot notice; production does not emit a
    // second session state just to clear it.
    await expect(page.locator(".f-idle")).toBeVisible();
  });
});
