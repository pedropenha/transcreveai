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
});
