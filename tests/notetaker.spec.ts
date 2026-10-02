import { test, expect, type Page } from "@playwright/test";
import { emitTauriEvent, installTauriMock } from "./helpers/tauri-mock";

const NOW = new Date();
const atToday = (hour: number, minute = 0) =>
  Math.floor(
    new Date(
      NOW.getFullYear(),
      NOW.getMonth(),
      NOW.getDate(),
      hour,
      minute,
    ).getTime() / 1000,
  );
const DAY = 24 * 3600;

type Status =
  | "recording"
  | "paused"
  | "processing"
  | "no_summary"
  | "failed"
  | "ready";

interface Row {
  id: string;
  title: string;
  startedAt: number;
  minutes: number;
  status: Status;
  dbStatus?: string;
  app?: { exe: string | null; name: string } | null;
  summary?: string | null;
}

const meeting = (row: Row) => ({
  id: row.id,
  title: row.title,
  app_exe: row.app?.exe ?? null,
  app_label: row.app?.name ?? null,
  detection: row.app ? "auto_prompt" : "manual",
  status:
    row.dbStatus ??
    (row.status === "recording" || row.status === "paused"
      ? row.status
      : row.status === "processing"
        ? "processing"
        : "ready"),
  started_at: row.startedAt,
  ended_at:
    row.status === "recording" || row.status === "paused"
      ? null
      : row.startedAt + row.minutes * 60,
  capture_system_audio: true,
  stt_provider_id: null,
  llm_provider_id: null,
  template_id: null,
  summary_md: row.summary ?? null,
  summary_status: row.status === "ready" ? "ready" : "pending",
  audio_dir: null,
  language: "en",
  error_code: null,
  source_app: row.app === undefined ? null : row.app,
  list_status: row.status,
});

const ROWS: Row[] = [
  {
    id: "m-zoom",
    title: "Product daily",
    startedAt: atToday(10),
    minutes: 32,
    status: "ready",
    app: { exe: "Zoom.exe", name: "Zoom" },
    summary:
      "The team reviewed the Notetaker.\n\n## Decisions\n- Keep Large v3 Turbo.\n\n## Next steps\n- Measure latency.",
  },
  {
    id: "m-obs",
    title: "Studio rehearsal",
    startedAt: atToday(8),
    minutes: 45,
    status: "processing",
    app: { exe: "obs64.exe", name: "OBS Studio" },
  },
  {
    id: "m-legal",
    title: "Legal alignment",
    startedAt: atToday(8) - DAY + 3600,
    minutes: 51,
    status: "no_summary",
    app: { exe: "Teams.exe", name: "Microsoft Teams" },
  },
  {
    id: "m-fail",
    title: "Camera test",
    startedAt: atToday(8) - DAY,
    minutes: 28,
    status: "failed",
    dbStatus: "error",
    app: { exe: "Discord.exe", name: "Discord" },
  },
  {
    id: "m-fail-sum",
    title: "Budget review",
    startedAt: atToday(8) - 3 * DAY,
    minutes: 20,
    status: "failed",
    dbStatus: "ready",
    app: null,
  },
];

const segments = [
  {
    id: "s1",
    meeting_id: "m-zoom",
    track: "system",
    speaker: "Others",
    start_ms: 12_000,
    end_ms: 15_000,
    text: "Starting with the Notetaker.",
    kind: "speech",
    is_final: true,
    excluded: false,
  },
  {
    id: "s2",
    meeting_id: "m-zoom",
    track: "mic",
    speaker: "You",
    start_ms: 107_000,
    end_ms: 110_000,
    text: "Without a GPU an hour takes twenty minutes.",
    kind: "speech",
    is_final: true,
    excluded: false,
  },
];

const rowOf = (page: Page, title: string) =>
  page.locator("li", { hasText: title });

async function openNotetaker(page: Page, rows: Row[] = ROWS) {
  let current = rows.map(meeting);
  const mock = await installTauriMock(page, {
    meeting_search: (args) => {
      const query = String(args.query ?? "").toLowerCase();
      return current.filter((m) => m.title.toLowerCase().includes(query));
    },
    meeting_current: null,
    meeting_source_icon: null,
    meeting_get: (args) => {
      const found = current.find((m) => m.id === args.id);
      return found
        ? {
            meeting: found,
            segments: found.id === "m-zoom" ? segments : [],
            notes_md: "",
            source_app: found.source_app,
            list_status: found.list_status,
          }
        : null;
    },
    meeting_export_markdown: "# Product daily",
    meeting_retry_processing: null,
    meeting_regenerate_summary: null,
    meeting_delete: (args) => {
      current = current.filter((m) => m.id !== args.id);
      return null;
    },
    meeting_rename: (args) => {
      current = current.map((m) =>
        m.id === args.id ? { ...m, title: String(args.title) } : m,
      );
      return current.find((m) => m.id === args.id);
    },
  });
  await page.addInitScript(() => {
    const copied: string[] = [];
    (window as unknown as { __copied: string[] }).__copied = copied;
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText: async (text: string) => void copied.push(text) },
    });
  });
  await page.goto("/");
  await page.getByTitle("Notetaker").click();
  return mock;
}

test.describe("Notetaker (mocked Tauri IPC)", () => {
  test("lists meetings grouped by day with the app logo and one state chip", async ({
    page,
  }) => {
    const mock = await openNotetaker(page);

    await expect(page.getByRole("heading", { name: "Today" })).toBeVisible();
    await expect(
      page.getByRole("heading", { name: "Yesterday" }),
    ).toBeVisible();
    const zoom = page.getByRole("button", { name: /Product daily/ });
    await expect(rowOf(page, "Product daily")).toContainText("Summary ready");
    // Known app: embedded logo, no IPC round trip.
    await expect(zoom.locator('.applogo[data-kind="img"]')).toBeVisible();
    // Unknown exe: backend asked once, null -> monogram.
    const obs = page.getByRole("button", { name: /Studio rehearsal/ });
    await expect(obs.locator('.applogo[data-kind="monogram"]')).toHaveText("O");
    // Manual meeting (no source app): Transcreve.ai mark.
    await expect(
      page
        .getByRole("button", { name: /Budget review/ })
        .locator('.applogo[data-kind="brand"]'),
    ).toBeVisible();

    await expect
      .poll(
        () => mock.calls.filter((c) => c.cmd === "meeting_source_icon").length,
      )
      .toBeGreaterThan(0);
    const iconCalls = mock.calls.filter((c) => c.cmd === "meeting_source_icon");
    expect(iconCalls.map((c) => c.args.id)).not.toContain("m-zoom");
    expect(iconCalls.map((c) => c.args.id)).not.toContain("m-legal");
  });

  test("uses the icon extracted by the backend, cached per executable", async ({
    page,
  }) => {
    const png =
      "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";
    const rows: Row[] = [
      {
        id: "a",
        title: "First",
        startedAt: atToday(9),
        minutes: 10,
        status: "ready",
        app: { exe: "obs64.exe", name: "OBS Studio" },
      },
      {
        id: "b",
        title: "Second",
        startedAt: atToday(7),
        minutes: 10,
        status: "ready",
        app: { exe: "OBS64.exe", name: "OBS Studio" },
      },
    ];
    const mock = await installTauriMock(page, {
      meeting_search: rows.map(meeting),
      meeting_current: null,
      meeting_source_icon: png,
    });
    await page.goto("/");
    await page.getByTitle("Notetaker").click();

    await expect(
      page
        .getByRole("button", { name: /First/ })
        .locator('.applogo[data-kind="img"] img'),
    ).toHaveAttribute("src", png);
    await expect(
      page
        .getByRole("button", { name: /Second/ })
        .locator('.applogo[data-kind="img"] img'),
    ).toHaveAttribute("src", png);
    expect(
      mock.calls.filter((c) => c.cmd === "meeting_source_icon").length,
    ).toBe(1);
  });

  test("row states: transcribing with live progress, no summary, failed", async ({
    page,
  }) => {
    await openNotetaker(page);

    const processing = rowOf(page, "Studio rehearsal");
    await expect(processing).toContainText("Transcribing");
    await emitTauriEvent(page, "meeting://progress", {
      meeting_id: "m-obs",
      step: "transcribing",
      pct: 64,
    });
    await expect(processing).toContainText("Transcribing 64%");
    await expect(
      page
        .getByRole("group", { name: "Meeting lists" })
        .getByRole("button", { name: /Transcribing/ }),
    ).toContainText("1");

    await expect(rowOf(page, "Legal alignment")).toContainText("No summary");
    await expect(rowOf(page, "Camera test")).toContainText("Failed");
  });

  test("the Transcribing tab only lists meetings being processed", async ({
    page,
  }) => {
    await openNotetaker(page);
    await page
      .getByRole("group", { name: "Meeting lists" })
      .getByRole("button", { name: /Transcribing/ })
      .click();
    await expect(
      page.getByRole("button", { name: /Studio rehearsal/ }),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: /Product daily/ }),
    ).toHaveCount(0);
  });

  test("retry routes by failure kind", async ({ page }) => {
    const mock = await openNotetaker(page);

    const camera = page.locator("li", { hasText: "Camera test" });
    await camera.getByRole("button", { name: "Try again" }).click();
    await expect
      .poll(() => mock.calls.find((c) => c.cmd === "meeting_retry_processing"))
      .toMatchObject({ args: { meetingId: "m-fail" } });

    const budget = page.locator("li", { hasText: "Budget review" });
    await budget.getByRole("button", { name: "Try again" }).click();
    await expect
      .poll(() =>
        mock.calls.find((c) => c.cmd === "meeting_regenerate_summary"),
      )
      .toMatchObject({ args: { meetingId: "m-fail-sum" } });
  });

  test("a no-summary row leads to the summary settings", async ({ page }) => {
    await openNotetaker(page);
    await page
      .locator("li", { hasText: "Legal alignment" })
      .getByRole("button", { name: "No summary" })
      .click();
    await expect(
      page.getByRole("navigation", { name: "Settings sections" }),
    ).toBeVisible();
  });

  test("selecting a meeting shows summary sections and the transcript excerpt", async ({
    page,
  }) => {
    const mock = await openNotetaker(page);
    await page.getByRole("button", { name: /Product daily/ }).click();

    const detail = page.getByRole("complementary", { name: "Meeting details" });
    await expect(
      detail.getByRole("heading", { name: "Product daily" }),
    ).toBeVisible();
    await expect(
      detail.getByRole("heading", { name: "Decisions" }),
    ).toBeVisible();
    await expect(detail.getByText("Keep Large v3 Turbo.")).toBeVisible();
    await expect(
      detail.getByRole("heading", { name: "Next steps" }),
    ).toBeVisible();
    await expect(detail.getByText("00:12")).toBeVisible();
    await expect(
      detail.getByText("Starting with the Notetaker."),
    ).toBeVisible();
    await expect(detail.getByText("01:47")).toBeVisible();

    await detail.getByRole("button", { name: "Open full transcript" }).click();
    await expect
      .poll(() => mock.calls.find((c) => c.cmd === "meeting_window_open"))
      .toMatchObject({ args: { meetingId: "m-zoom" } });

    await detail.getByRole("button", { name: "Copy as Markdown" }).click();
    await expect
      .poll(() =>
        page.evaluate(
          () => (window as unknown as { __copied: string[] }).__copied,
        ),
      )
      .toContain("# Product daily");

    await page.keyboard.press("Escape");
    await expect(detail).toHaveCount(0);
  });

  test("rename and delete work from the details", async ({ page }) => {
    const mock = await openNotetaker(page);
    await page.getByRole("button", { name: /Product daily/ }).click();
    const detail = page.getByRole("complementary", { name: "Meeting details" });

    await detail.getByRole("button", { name: "Rename meeting" }).click();
    const input = detail.getByRole("textbox", { name: "Rename meeting" });
    await input.fill("Weekly product sync");
    await input.press("Enter");
    await expect
      .poll(() => mock.calls.find((c) => c.cmd === "meeting_rename"))
      .toMatchObject({ args: { id: "m-zoom", title: "Weekly product sync" } });
    await expect(
      page.getByRole("button", { name: /Weekly product sync/ }),
    ).toBeVisible();

    await detail.getByRole("button", { name: "Delete meeting" }).click();
    await detail.getByRole("button", { name: "Confirm delete" }).click();
    await expect
      .poll(() => mock.calls.find((c) => c.cmd === "meeting_delete"))
      .toMatchObject({ args: { id: "m-zoom" } });
    await expect(
      page.getByRole("button", { name: /Weekly product sync/ }),
    ).toHaveCount(0);
    await expect(detail).toHaveCount(0);
  });

  test("search filters through meeting_search", async ({ page }) => {
    const mock = await openNotetaker(page);
    await page.getByRole("button", { name: "Search meetings" }).click();
    await page.getByRole("searchbox").fill("legal");

    await expect(
      page.getByRole("button", { name: /Legal alignment/ }),
    ).toBeVisible();
    await expect(
      page.getByRole("button", { name: /Product daily/ }),
    ).toHaveCount(0);
    expect(
      mock.calls.some(
        (c) => c.cmd === "meeting_search" && c.args.query === "legal",
      ),
    ).toBe(true);

    await page.getByRole("searchbox").fill("zzz");
    await expect(
      page.getByText("No meetings match your search."),
    ).toBeVisible();
  });

  test("Agora block shows the live meeting and controls it", async ({
    page,
  }) => {
    const live: Row = {
      id: "m-live",
      title: "Q4 planning",
      startedAt: atToday(11),
      minutes: 0,
      status: "recording",
      app: { exe: "Teams.exe", name: "Microsoft Teams" },
    };
    const mock = await openNotetaker(page, [live, ...ROWS]);
    const block = page.getByRole("region", { name: "Recording now" });
    await expect(block).toContainText("Q4 planning");
    await expect(block).toContainText("Mic + system audio");

    await emitTauriEvent(page, "meeting://state", {
      meeting_id: "m-live",
      status: "recording",
      elapsed_ms: 724_000,
    });
    await expect(block.getByRole("timer")).toHaveText("12:04");

    await block.getByRole("button", { name: "Pause" }).click();
    await expect
      .poll(() => mock.calls.some((c) => c.cmd === "meeting_pause"))
      .toBe(true);
    await block.getByRole("button", { name: "Stop" }).click();
    await expect
      .poll(() => mock.calls.some((c) => c.cmd === "meeting_stop"))
      .toBe(true);
    // The live meeting is not in the past list.
    await expect(page.getByRole("list").getByText("Q4 planning")).toHaveCount(
      0,
    );
  });

  test("empty state offers to start the Notetaker", async ({ page }) => {
    const mock = await openNotetaker(page, []);
    const empty = page.getByTestId("notetaker-empty");
    await expect(empty).toContainText("No meetings recorded yet");
    await empty.getByRole("button", { name: "Start Notetaker" }).click();
    await expect
      .poll(() => mock.calls.find((c) => c.cmd === "meeting_start"))
      .toMatchObject({ args: { micOnly: false } });
  });

  test("the start menu offers computer call and in person", async ({
    page,
  }) => {
    const mock = await openNotetaker(page);
    await page
      .getByRole("banner")
      .getByRole("button", { name: "Start Notetaker" })
      .click();
    await page.getByRole("menuitem", { name: "In person" }).click();
    await expect
      .poll(() => mock.calls.find((c) => c.cmd === "meeting_start"))
      .toMatchObject({ args: { micOnly: true } });
  });
});
