import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { LANGUAGE_METADATA, resolveSupportedLanguage } from "./languages";

// ADR-0002: v1 ships only English and Brazilian Portuguese UI locales.
const LOCALES_DIR = path.join(
  path.dirname(fileURLToPath(import.meta.url)),
  "locales",
);

const localeDirs = fs
  .readdirSync(LOCALES_DIR, { withFileTypes: true })
  .filter((entry) => entry.isDirectory())
  .map((entry) => entry.name)
  .sort();

assert.deepEqual(
  localeDirs,
  ["en", "pt-BR"],
  "only en and pt-BR locales may ship",
);

assert.deepEqual(
  Object.keys(LANGUAGE_METADATA).sort(),
  localeDirs,
  "every shipped locale needs metadata (and vice versa)",
);

// Language resolution: exact → primary subtag → null (caller falls back to en).
const supported = localeDirs;
for (const [input, expected] of [
  ["en", "en"],
  ["en-US", "en"],
  ["pt", "pt-BR"],
  ["pt-BR", "pt-BR"],
  ["pt_br", "pt-BR"],
  ["pt-PT", "pt-BR"],
  ["de-DE", null],
  ["zh-Hant-TW", null],
  [null, null],
  [undefined, null],
] as const) {
  assert.equal(
    resolveSupportedLanguage(input, supported),
    expected,
    `${input} should resolve to ${expected}`,
  );
}

type TranslationData = Record<string, unknown>;
const flatten = (
  obj: TranslationData,
  prefix: string[] = [],
  out: Record<string, unknown> = {},
): Record<string, unknown> => {
  for (const key of Object.keys(obj)) {
    const value = obj[key];
    const next = [...prefix, key];
    if (value !== null && typeof value === "object" && !Array.isArray(value)) {
      flatten(value as TranslationData, next, out);
    } else {
      out[next.join(".")] = value;
    }
  }
  return out;
};

const load = (locale: string): Record<string, unknown> =>
  flatten(
    JSON.parse(
      fs.readFileSync(
        path.join(LOCALES_DIR, locale, "translation.json"),
        "utf8",
      ),
    ) as TranslationData,
  );

const en = load("en");
const ptBR = load("pt-BR");

const missing = Object.keys(en).filter((key) => !(key in ptBR));
const extra = Object.keys(ptBR).filter((key) => !(key in en));

assert.deepEqual(missing, [], `pt-BR is missing keys: ${missing.join(", ")}`);
assert.deepEqual(
  extra,
  [],
  `pt-BR has keys not present in en: ${extra.join(", ")}`,
);

console.log("i18n locales: all assertions passed");
