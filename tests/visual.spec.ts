import { test } from "@playwright/test";
import { installTauriMock } from "./helpers/tauri-mock";

/**
 * Visual regression captures for the Hub ("Papel & Anil" proposal).
 * Opt-in (CAPTURE_SCREENS=1) because it writes PNGs into docs/design/screens/
 * for human comparison against docs/design/proposta-ui.html.
 */
const NOW = Math.floor(Date.now() / 1000);
const SAMPLE = [
  [
    0,
    "Slack",
    "Ok, pode seguir com a versão sem legenda embutida. A legenda fica num arquivo .srt separado e o resto continua igual.",
  ],
  [
    3600,
    "Notion",
    "Na parte de acesso, separa por equipe: financeiro, RH e TI. Depois coloca pedagógico e administrativo.",
  ],
  [
    90_000,
    "WhatsApp",
    "Oi! Consigo sim, amanhã às dez fica ótimo. Levo o roteiro impresso e a gente revisa juntos.",
  ],
].map(([ago, app, text], i) => ({
  id: i + 1,
  file_name: `d${i}.wav`,
  timestamp: NOW - (ago as number),
  saved: false,
  title: "",
  transcription_text: "",
  post_processed_text: null,
  post_process_prompt: null,
  post_process_requested: false,
  mode: "dictation",
  duration_ms: 5000,
  app_exe: `${app}.exe`,
  app_name: app,
  stt_provider_id: null,
  llm_provider_id: null,
  language: "pt",
  raw_text: text,
  final_text: text,
  status: "inserted",
  error_code: null,
  latency_json: "{}",
  audio_available: true,
  word_count: 20,
}));
const STATS = {
  words_today: 40,
  words_week: 90,
  words_total: 3933,
  duration_ms_total: 1_120_000,
  words_per_minute: 211,
  seconds_saved: 98 * 60,
  provider_usage: [],
};

const atToday = (hour: number, minute = 0) => {
  const d = new Date();
  return Math.floor(
    new Date(
      d.getFullYear(),
      d.getMonth(),
      d.getDate(),
      hour,
      minute,
    ).getTime() / 1000,
  );
};
const MEETING_BASE = {
  detection: "auto_prompt",
  capture_system_audio: true,
  stt_provider_id: null,
  llm_provider_id: null,
  template_id: null,
  audio_dir: null,
  language: "pt",
  error_code: null,
  summary_status: "ready",
};
const MEETINGS = [
  ["m1", "Daily de produto", "Zoom.exe", "Zoom", 10, 32, "ready", "ready"],
  [
    "m2",
    "Entrevista · Designer sênior",
    "chrome.exe",
    "Google Meet",
    8,
    45,
    "processing",
    "processing",
  ],
  [
    "m3",
    "Alinhamento com o jurídico",
    "Teams.exe",
    "Microsoft Teams",
    -16,
    51,
    "ready",
    "no_summary",
  ],
  [
    "m4",
    "Teste final da câmera",
    "Discord.exe",
    "Discord",
    -5,
    28,
    "ready",
    "ready",
  ],
  [
    "m5",
    "Gravação do estúdio",
    "obs64.exe",
    "OBS Studio",
    -30,
    20,
    "error",
    "failed",
  ],
].map(([id, title, exe, name, hour, minutes, status, list]) => {
  const startedAt =
    (hour as number) >= 0
      ? atToday(hour as number)
      : atToday(24 + (hour as number)) - 24 * 3600;
  return {
    ...MEETING_BASE,
    id,
    title,
    app_exe: exe,
    app_label: name,
    status,
    started_at: startedAt,
    ended_at: startedAt + (minutes as number) * 60,
    summary_md:
      list === "ready"
        ? "O time revisou o andamento.\n\n## Decisões\n- Modelo padrão continua Large v3 Turbo.\n\n## Próximos passos\n- Medir a latência."
        : null,
    source_app: { exe, name },
    list_status: list,
  };
});

const WIDTHS = [900, 1280, 1600] as const;
const SCHEMES = ["light", "dark"] as const;

test.describe("hub screenshots", () => {
  test.skip(!process.env.CAPTURE_SCREENS, "set CAPTURE_SCREENS=1 to capture");

  for (const scheme of SCHEMES) {
    for (const width of WIDTHS) {
      test(`hub home ${scheme} ${width}px`, async ({ page }) => {
        await installTauriMock(page, {
          search_history_entries: { entries: SAMPLE, has_more: false },
          get_history_statistics: STATS,
        });
        await page.emulateMedia({ colorScheme: scheme });
        await page.setViewportSize({ width, height: 800 });
        await page.goto("/");
        await page.waitForSelector(".hist-row");
        await page.waitForTimeout(400);
        await page.screenshot({
          path: `docs/design/screens/${process.env.SCREEN_PREFIX ?? "hub"}-${scheme}-${width}.png`,
        });
      });
    }
  }

  for (const scheme of SCHEMES) {
    for (const width of WIDTHS) {
      test(`notetaker ${scheme} ${width}px`, async ({ page }) => {
        await installTauriMock(page, {
          meeting_search: MEETINGS,
          meeting_current: null,
          meeting_source_icon: null,
          meeting_get: (args) => {
            const found = MEETINGS.find((m) => m.id === args.id);
            return found
              ? {
                  meeting: found,
                  segments: [
                    {
                      id: "s1",
                      meeting_id: found.id,
                      track: "system",
                      speaker: "Outros",
                      start_ms: 12_000,
                      end_ms: 16_000,
                      text: "Bom, começando pelo Notetaker: a captura do sistema ficou estável.",
                      kind: "speech",
                      is_final: true,
                      excluded: false,
                    },
                  ],
                  notes_md: "",
                  source_app: found.source_app,
                  list_status: found.list_status,
                }
              : null;
          },
        });
        await page.emulateMedia({ colorScheme: scheme });
        await page.setViewportSize({ width, height: 800 });
        await page.goto("/");
        await page.getByTitle("Notetaker").click();
        await page.getByRole("button", { name: /Daily de produto/ }).click();
        await page.waitForSelector(".nt-detail .nt-md");
        await page.waitForTimeout(400);
        await page.screenshot({
          path: `docs/design/screens/${process.env.SCREEN_PREFIX ?? "hub"}-notetaker-${scheme}-${width}.png`,
        });
      });
    }
  }
});
