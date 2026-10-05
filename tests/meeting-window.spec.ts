import { test, expect, type Page } from "@playwright/test";
import { emitTauriEvent, installTauriMock } from "./helpers/tauri-mock";
import {
  startCorrectionCoverage,
  stopCorrectionCoverage,
} from "./helpers/correction-coverage";

test.beforeEach(async ({ page }) => startCorrectionCoverage(page));
test.afterEach(async ({ page }, testInfo) =>
  stopCorrectionCoverage(page, testInfo),
);

const segment = (id: string, text: string) => ({
  id: `${id}-${text}`,
  meeting_id: id,
  track: "mic",
  speaker: null,
  start_ms: 0,
  end_ms: 1000,
  text,
  kind: "speech",
  is_final: true,
  excluded: false,
});

const detail = (id: string, texts: string[] = [], notes = "") => ({
  meeting: {
    id,
    title: `Meeting ${id}`,
    status: "ready",
    started_at: 100,
    ended_at: 160,
    detection: "manual",
    app_label: null,
    summary_md: `Summary ${id}`,
    summary_status: "ready",
    llm_provider_id: null,
  },
  segments: texts.map((text) => segment(id, text)),
  notes_md: notes,
});

const open = async (page: Page, id: string) => {
  await emitTauriEvent(page, "meeting://open", { meeting_id: id });
  await expect(page.getByTitle(`Meeting ${id}`, { exact: true })).toBeVisible();
};

test("retargeted meeting window resets transcript and pending notes and summary edits", async ({
  page,
}) => {
  const mock = await installTauriMock(page, {
    meeting_get: (args) =>
      detail(
        String(args.id),
        args.id === "A" ? ["Old conversation"] : [],
        args.id === "A" ? "Notes A" : "",
      ),
    llm_summary_status: { enabled: true },
  });
  await page.goto("/src/meeting/index.html?meeting_id=A");
  await expect(page.getByRole("textbox", { name: "My notes" })).toHaveValue(
    "Notes A",
  );
  await page.getByRole("textbox", { name: "My notes" }).fill("Unsaved A");
  await page.getByRole("tab", { name: "Summary" }).click();
  await page.getByRole("button", { name: "Edit", exact: true }).click();
  await page
    .getByRole("textbox", { name: "Summary", exact: true })
    .fill("Unsaved summary A");
  await open(page, "B");
  await page.getByRole("tab", { name: "My notes" }).click();
  await expect(page.getByRole("textbox", { name: "My notes" })).toHaveValue("");
  await page.getByRole("tab", { name: "Transcript" }).click();
  await expect(page.getByText("Old conversation", { exact: true })).toHaveCount(
    0,
  );
  await emitTauriEvent(
    page,
    "meeting://segment",
    segment("A", "Late old event"),
  );
  await emitTauriEvent(
    page,
    "meeting://segment",
    segment("B", "New conversation"),
  );
  await expect(
    page.getByText("New conversation", { exact: true }),
  ).toBeVisible();
  await expect(page.getByText("Late old event", { exact: true })).toHaveCount(
    0,
  );
  await page.waitForTimeout(1000);
  expect(
    mock.calls.filter((c) =>
      ["meeting_notes_update", "meeting_summary_update"].includes(c.cmd),
    ),
  ).toEqual([]);
});

test("late hydration from previous meeting cannot replace the new meeting", async ({
  page,
}) => {
  let release: (() => void) | undefined;
  let delay = false;
  const mock = await installTauriMock(page, {
    meeting_get: async (args) => {
      if (args.id === "A" && delay)
        await new Promise<void>((resolve) => {
          release = resolve;
        });
      return detail(String(args.id), [
        args.id === "A" ? "Old snapshot" : "Current snapshot",
      ]);
    },
    llm_summary_status: { enabled: true },
  });
  await page.goto("/src/meeting/index.html?meeting_id=A");
  await expect(page.getByTitle("Meeting A", { exact: true })).toBeVisible();
  delay = true;
  await emitTauriEvent(page, "meeting://state", {
    meeting_id: "A",
    status: "ready",
    elapsed_ms: 60000,
  });
  await expect
    .poll(
      () =>
        mock.calls.filter((c) => c.cmd === "meeting_get" && c.args.id === "A")
          .length,
    )
    .toBeGreaterThan(1);
  await expect.poll(() => Boolean(release)).toBe(true);
  await open(page, "B");
  release?.();
  await page.getByRole("tab", { name: "Transcript" }).click();
  await expect(
    page.getByText("Current snapshot", { exact: true }),
  ).toBeVisible();
  await page.waitForTimeout(200);
  await expect(page.getByTitle("Meeting B", { exact: true })).toBeVisible();
  await expect(page.getByText("Old snapshot", { exact: true })).toHaveCount(0);
});

test("same meeting hydration preserves segments arriving during the request", async ({
  page,
}) => {
  let release: (() => void) | undefined;
  await installTauriMock(page, {
    meeting_get: async () => {
      await new Promise<void>((resolve) => {
        release = resolve;
      });
      return detail("A", ["Stored segment"]);
    },
  });
  await page.goto("/src/meeting/index.html?meeting_id=A");
  await expect.poll(() => Boolean(release)).toBe(true);
  await emitTauriEvent(page, "meeting://segment", segment("A", "Live segment"));
  release?.();
  await expect(page.getByTitle("Meeting A", { exact: true })).toBeVisible();
  await page.getByRole("tab", { name: "Transcript" }).click();
  await expect(page.getByText("Stored segment", { exact: true })).toBeVisible();
  await expect(page.getByText("Live segment", { exact: true })).toBeVisible();
});

test("explicit open wins over delayed current-meeting resolution", async ({
  page,
}) => {
  const releases: Array<() => void> = [];
  await installTauriMock(page, {
    meeting_current: async () => {
      await new Promise<void>((resolve) => releases.push(resolve));
      return { meeting_id: "A" };
    },
    meeting_get: (args) => detail(String(args.id)),
  });
  await page.goto("/src/meeting/index.html");
  await expect.poll(() => releases.length).toBeGreaterThan(0);
  await open(page, "B");
  releases.forEach((release) => release());
  await page.waitForTimeout(200);
  await expect(page.getByTitle("Meeting B", { exact: true })).toBeVisible();
});

test("returning to a meeting reloads its own saved notes, summary and transcript", async ({
  page,
}) => {
  const mock = await installTauriMock(page, {
    meeting_get: (args) =>
      detail(
        String(args.id),
        [`Conversation ${args.id}`],
        `Saved notes ${args.id}`,
      ),
    llm_summary_status: { enabled: true },
  });
  await page.goto("/src/meeting/index.html?meeting_id=A");
  await expect(page.getByTitle("Meeting A", { exact: true })).toBeVisible();
  await open(page, "B");
  await page.getByRole("textbox", { name: "My notes" }).fill("Edited notes B");
  await expect
    .poll(() => mock.calls.filter((c) => c.cmd === "meeting_notes_update"))
    .toEqual([
      {
        cmd: "meeting_notes_update",
        args: { id: "B", bodyMd: "Edited notes B" },
      },
    ]);
  await page.getByRole("tab", { name: "Summary" }).click();
  await page.getByRole("button", { name: "Edit", exact: true }).click();
  await page
    .getByRole("textbox", { name: "Summary", exact: true })
    .fill("Edited summary B");
  await expect
    .poll(() => mock.calls.filter((c) => c.cmd === "meeting_summary_update"))
    .toEqual([
      {
        cmd: "meeting_summary_update",
        args: { id: "B", summaryMd: "Edited summary B" },
      },
    ]);
  await open(page, "A");
  await expect(page.getByRole("textbox", { name: "My notes" })).toHaveValue(
    "Saved notes A",
  );
  await page.getByRole("tab", { name: "Summary" }).click();
  await expect(page.getByText("Summary A", { exact: true })).toBeVisible();
  await page.getByRole("tab", { name: "Transcript" }).click();
  await expect(page.getByText("Conversation A", { exact: true })).toBeVisible();
  await expect(page.getByText("Conversation B", { exact: true })).toHaveCount(
    0,
  );
  const before = mock.calls.filter((c) => c.cmd === "meeting_get").length;
  await emitTauriEvent(page, "meeting://state", {
    meeting_id: "A",
    status: "ready",
    elapsed_ms: 60000,
  });
  await expect
    .poll(() => mock.calls.filter((c) => c.cmd === "meeting_get").length)
    .toBe(before + 1);
});
