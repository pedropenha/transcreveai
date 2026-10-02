import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import {
  contrastRatio,
  extractRuleBody,
  hexToRgb,
  luminance,
  readHexTokens,
} from "./contrast";

// ---- helpers ------------------------------------------------------------

assert.equal(contrastRatio("#000000", "#ffffff").toFixed(2), "21.00");
assert.equal(contrastRatio("#777", "#777").toFixed(2), "1.00");
assert.deepEqual(hexToRgb("#f00"), [1, 0, 0]);
assert.ok(luminance("#ffffff") > luminance("#808080"));
assert.throws(() => hexToRgb("rgb(0 0 0)"), /Not a hex color/);

assert.equal(
  extractRuleBody("a { --x: #fff; } :root { --y: #000; }", ":root")?.trim(),
  "--y: #000;",
);
assert.equal(extractRuleBody("a { --x: #fff; }", ":root"), null);
// A selector appearing inside another selector must not match.
assert.equal(
  extractRuleBody(':root[data-theme="dark"] { --y: #000; }', ":root"),
  null,
);
assert.deepEqual(readHexTokens("--a: #ABC; --b: 4px; --c: var(--a);"), {
  "--a": "#abc",
});

// ---- the shipped palette -----------------------------------------------

const css = readFileSync(
  fileURLToPath(new URL("./theme.css", import.meta.url)),
  "utf8",
);

const lightBody = extractRuleBody(css, ":root");
const darkForced = extractRuleBody(css, ':root[data-theme="dark"]');
const darkSystem = extractRuleBody(css, ':root:not([data-theme="light"])');
assert.ok(lightBody, "theme.css must define a :root block");
assert.ok(darkForced, 'theme.css must define :root[data-theme="dark"]');
assert.ok(darkSystem, "theme.css must define the prefers-color-scheme block");

const overlay = readHexTokens(lightBody);
const light = overlay;
const dark = { ...light, ...readHexTokens(darkForced) };

// The OS-driven dark block and the forced one must carry the same palette,
// otherwise "system" and "dark" would render differently.
assert.deepEqual(
  readHexTokens(darkSystem),
  readHexTokens(darkForced),
  "dark palette must be identical for system and forced dark",
);

type Pair = readonly [fg: string, bg: string, use: string, min: number];

// Mirrors the table in docs/design/proposta-ui.html (WCAG 2.2 AA:
// 4.5:1 for text, 3:1 for control borders and focus indicators).
const THEMED_PAIRS: readonly Pair[] = [
  ["--text", "--panel", "main text", 4.5],
  ["--muted", "--panel", "secondary text", 4.5],
  ["--subtle", "--panel", "metadata, times", 4.5],
  ["--muted", "--canvas", "labels on the canvas", 4.5],
  ["--subtle", "--canvas", "rail group label", 4.5],
  ["--subtle", "--hover", "time on a hovered row", 4.5],
  ["--accent", "--panel", "link, active icon", 4.5],
  ["--on-accent", "--accent", "primary button", 4.5],
  ["--accent-ink", "--accent-soft", "chip / soft button / selected", 4.5],
  ["--urucum-ink", "--urucum-soft", "record button, highlight chip", 4.5],
  ["--success", "--success-soft", "success chip", 4.5],
  ["--warning", "--warning-soft", "warning chip", 4.5],
  ["--error", "--error-soft", "error chip", 4.5],
  ["--error", "--panel", "field error message", 4.5],
  ["--line-strong", "--panel", "input border, toggle off", 3],
  ["--accent", "--panel", "focus ring", 3],
];

const OVERLAY_PAIRS: readonly Pair[] = [
  ["--ov-text", "--ov-bg", "Flow Bar / toast", 4.5],
  ["--ov-muted", "--ov-bg", "toast secondary", 4.5],
  ["--ov-cta-ink", "--ov-cta", "toast button", 4.5],
  ["--ov-accent", "--ov-bg", "overlay accent", 4.5],
  ["--ov-rec", "--ov-bg", "overlay recording signal", 4.5],
  ["--ov-processing", "--ov-bg", "overlay processing", 4.5],
  ["--ov-success", "--ov-bg", "overlay success", 4.5],
  ["--ov-error", "--ov-bg", "overlay error", 4.5],
  ["--ov-command", "--ov-bg", "overlay command", 4.5],
];

function check(
  themeName: string,
  tokens: Readonly<Record<string, string>>,
  pairs: readonly Pair[],
): void {
  for (const [fg, bg, use, min] of pairs) {
    const foreground = tokens[fg];
    const background = tokens[bg];
    assert.ok(foreground, `${themeName}: missing token ${fg}`);
    assert.ok(background, `${themeName}: missing token ${bg}`);
    const ratio = contrastRatio(foreground, background);
    assert.ok(
      ratio >= min,
      `${themeName}: ${fg} on ${bg} (${use}) is ${ratio.toFixed(2)}:1, needs ${min}:1`,
    );
  }
}

check("light", light, THEMED_PAIRS);
check("dark", dark, THEMED_PAIRS);
check("overlay", overlay, OVERLAY_PAIRS);

// Anil on the dark panel flips to ink on the accent: the proposal quotes ~8:1.
assert.ok(contrastRatio(dark["--on-accent"], dark["--accent"]) >= 7);

// Light and dark must actually differ where it matters, and every semantic
// token the dark block overrides must exist in the light block.
assert.notEqual(light["--canvas"], dark["--canvas"]);
for (const name of Object.keys(readHexTokens(darkForced))) {
  assert.ok(name in light, `dark overrides unknown light token ${name}`);
}

console.log("theme tokens: contrast assertions passed");
