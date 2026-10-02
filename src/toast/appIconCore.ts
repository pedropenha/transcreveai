/**
 * Pure part of the meeting-toast app icon (F008): label → bundled-logo slug
 * and the resolution order. Kept free of Vite-only APIs (`import.meta.glob`)
 * so it is unit-tested under bun; `appIcon.ts` wires the bundled files in.
 */

export type ResolvedAppIcon =
  | { kind: "img"; src: string }
  | { kind: "fallback" };

/** Whole-word patterns on the lower-cased label → slug of
 *  `assets/app-icons/<slug>.svg` ("meeting" or "steams" must not match). */
const SLUG_RULES: ReadonlyArray<readonly [RegExp, string]> = [
  [/\bzoom\b/, "zoom"],
  [/\bteams\b/, "teams"],
  [/\bwebex\b/, "webex"],
  [/\bdiscord\b/, "discord"],
  [/\bslack\b/, "slack"],
  [/\bmeet\b/, "meet"],
];

export function appIconSlug(label: string): string | null {
  const normalized = label.trim().toLowerCase();
  if (normalized === "") return null;
  const rule = SLUG_RULES.find(([pattern]) => pattern.test(normalized));
  return rule ? rule[1] : null;
}

const PNG_DATA_URI = /^data:image\/png;base64,[A-Za-z0-9+/=]+$/;

export function createAppIconResolver(
  bundledBySlug: Readonly<Record<string, string>>,
): (label: string, icon?: string | null) => ResolvedAppIcon {
  return (label, icon) => {
    if (typeof icon === "string" && PNG_DATA_URI.test(icon)) {
      return { kind: "img", src: icon };
    }
    const slug = appIconSlug(label);
    const bundled = slug === null ? undefined : bundledBySlug[slug];
    return bundled ? { kind: "img", src: bundled } : { kind: "fallback" };
  };
}
