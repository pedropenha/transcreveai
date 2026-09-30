// Regenerates every app and tray icon from the sound-bar mark shared with the
// in-app logo (src/components/icons). Run from the repo root:
//   bun scripts/generate-brand-icons.ts
// Writes SVG sources to src-tauri/icons/source/, bundle icons to
// src-tauri/icons/ (via `tauri icon`) and 64 px tray icons to src-tauri/resources/.
import { mkdirSync, mkdtempSync, copyFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { SOUND_BAR_HEIGHTS } from "../src/lib/constants/app";
import {
  layoutSoundBars,
  type SoundBarsBox,
} from "../src/components/icons/soundBars";

const ROOT = join(import.meta.dir, "..");
const SOURCE_DIR = join(ROOT, "src-tauri", "icons", "source");
const RESOURCES_DIR = join(ROOT, "src-tauri", "resources");

const BRAND_BACKGROUND = "#da5893"; // --color-background-ui
const BRAND_BARS_ON_BACKGROUND = "#ffffff";
const TRAY_COLORED = "#f28cbb"; // --dark-color-logo-primary
const TRAY_SIZE = 64;

const bars = (box: SoundBarsBox, fill: string): string =>
  layoutSoundBars(SOUND_BAR_HEIGHTS, box)
    .map(
      (b) =>
        `<rect x="${b.x.toFixed(2)}" y="${b.y.toFixed(2)}" width="${b.width.toFixed(2)}" height="${b.height.toFixed(2)}" rx="${(b.width / 2).toFixed(2)}" fill="${fill}"/>`,
    )
    .join("");

const svg = (size: number, body: string): string =>
  `<svg xmlns="http://www.w3.org/2000/svg" width="${size}" height="${size}" viewBox="0 0 ${size} ${size}">${body}</svg>\n`;

const appIcon = (): string =>
  svg(
    1024,
    `<rect x="64" y="64" width="896" height="896" rx="200" fill="${BRAND_BACKGROUND}"/>` +
      bars(
        { x: 232, width: 560, centerY: 512, maxHeight: 600, gapRatio: 0.5 },
        BRAND_BARS_ON_BACKGROUND,
      ),
  );

const trayIdle = (color: string): string =>
  svg(
    TRAY_SIZE,
    bars({ x: 6, width: 52, centerY: 32, maxHeight: 52, gapRatio: 0.5 }, color),
  );

// Idle mark shrunk to the top-left with an exclamation badge knocked out of a
// circle in the bottom-right, matching the inherited warning icons' layout.
const trayWarning = (color: string): string =>
  svg(
    TRAY_SIZE,
    bars(
      { x: 4, width: 36, centerY: 24, maxHeight: 40, gapRatio: 0.5 },
      color,
    ) +
      `<mask id="bang"><rect width="64" height="64" fill="#fff"/>` +
      `<rect x="47" y="37" width="4" height="13" rx="2" fill="#000"/>` +
      `<circle cx="49" cy="55" r="2.4" fill="#000"/></mask>` +
      `<circle cx="49" cy="48" r="15" fill="${color}" mask="url(#bang)"/>`,
  );

const writeSvg = async (name: string, content: string): Promise<string> => {
  const path = join(SOURCE_DIR, `${name}.svg`);
  await Bun.write(path, content);
  return path;
};

const tauriIcon = (args: string[]): void => {
  const result = Bun.spawnSync(["bun", "run", "tauri", "icon", ...args], {
    cwd: ROOT,
    stdout: "inherit",
    stderr: "inherit",
  });
  if (result.exitCode !== 0) {
    throw new Error(
      `tauri icon ${args.join(" ")} failed with exit code ${result.exitCode}`,
    );
  }
};

const rasterizeTray = (svgPath: string, resourceName: string): void => {
  const out = mkdtempSync(join(tmpdir(), "brand-icon-"));
  try {
    tauriIcon([svgPath, "-o", out, "-p", String(TRAY_SIZE)]);
    copyFileSync(
      join(out, `${TRAY_SIZE}x${TRAY_SIZE}.png`),
      join(RESOURCES_DIR, resourceName),
    );
  } finally {
    rmSync(out, { recursive: true, force: true });
  }
};

mkdirSync(SOURCE_DIR, { recursive: true });

tauriIcon([await writeSvg("app-icon", appIcon())]);

const trayIcons: Array<[name: string, content: string, resource: string]> = [
  ["tray-idle-light", trayIdle("#ffffff"), "tray_idle.png"],
  ["tray-idle-dark", trayIdle("#000000"), "tray_idle_dark.png"],
  ["tray-idle-colored", trayIdle(TRAY_COLORED), "idle.png"],
  ["tray-idle-warning-light", trayWarning("#ffffff"), "tray_idle_warning.png"],
  [
    "tray-idle-warning-dark",
    trayWarning("#000000"),
    "tray_idle_warning_dark.png",
  ],
];
for (const [name, content, resource] of trayIcons) {
  rasterizeTray(await writeSvg(name, content), resource);
}

console.log("brand icons regenerated");
