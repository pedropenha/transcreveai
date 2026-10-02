/**
 * WCAG 2.2 contrast helpers and a minimal theme.css token reader.
 *
 * Used by theme.test.ts to keep the "Papel & Anil" palette AA-compliant:
 * the proposal (docs/design/proposta-ui.html) computes the same pairs live in
 * the browser; this module repeats that calculation against the shipped CSS so
 * a palette edit that drops below 4.5:1 / 3:1 fails the unit suite.
 */

export type TokenMap = Readonly<Record<string, string>>;

const HEX = /^#([0-9a-f]{3}|[0-9a-f]{6})$/i;

/** Parse `#rgb` / `#rrggbb` into sRGB channels in 0..1. */
export function hexToRgb(hex: string): [number, number, number] {
  if (!HEX.test(hex)) throw new Error(`Not a hex color: "${hex}"`);
  const digits = hex.slice(1);
  const full =
    digits.length === 3
      ? [...digits].map((char) => char + char).join("")
      : digits;
  const channel = (offset: number) =>
    parseInt(full.slice(offset, offset + 2), 16) / 255;
  return [channel(0), channel(2), channel(4)];
}

/** Relative luminance per WCAG 2.x. */
export function luminance(hex: string): number {
  const [r, g, b] = hexToRgb(hex).map((value) =>
    value <= 0.03928 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4,
  );
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/** Contrast ratio between two hex colors (1..21). */
export function contrastRatio(foreground: string, background: string): number {
  const a = luminance(foreground);
  const b = luminance(background);
  return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
}

/**
 * Body of the first `selector { ... }` rule whose selector text is exactly
 * `selector`. Theme blocks hold only declarations (no nested rules), so the
 * first closing brace ends the body. Returns null when the rule is absent.
 */
export function extractRuleBody(css: string, selector: string): string | null {
  const stripped = css.replace(/\/\*[\s\S]*?\*\//g, "");
  let from = 0;
  for (;;) {
    const at = stripped.indexOf(selector, from);
    if (at === -1) return null;
    const before = stripped.slice(0, at).trimEnd();
    const after = stripped.slice(at + selector.length).trimStart();
    const startsRule = before === "" || /[{};]$/.test(before);
    if (startsRule && after.startsWith("{")) {
      const open = stripped.indexOf("{", at);
      return stripped.slice(open + 1, stripped.indexOf("}", open));
    }
    from = at + selector.length;
  }
}

/** `--name: #hex;` declarations of a rule body (non-hex values are ignored). */
export function readHexTokens(body: string): TokenMap {
  const tokens: Record<string, string> = {};
  for (const match of body.matchAll(
    /(--[\w-]+)\s*:\s*(#[0-9a-fA-F]{3,6})\s*;/g,
  )) {
    tokens[match[1]] = match[2].toLowerCase();
  }
  return tokens;
}
