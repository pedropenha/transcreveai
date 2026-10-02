/**
 * Pure helpers for the Dicionário screen (F004 dictionary stage, FR-010-28):
 * term normalisation, search and the before/after preview of crutch removal.
 */

export const MAX_TERM_LENGTH = 50;

/** Collapse whitespace; the single normalisation shared by every list. */
export function normalizeTerm(value: string): string {
  return value.replace(/\s+/g, " ").trim();
}

/** Case-insensitive substring search that keeps the original order. */
export function filterTerms(terms: readonly string[], query: string): string[] {
  const needle = normalizeTerm(query).toLocaleLowerCase();
  return needle === ""
    ? [...terms]
    : terms.filter((term) => term.toLocaleLowerCase().includes(needle));
}

const escapeRegExp = (value: string) =>
  value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/**
 * Approximate "after" text for the preview: removes each crutch as a whole
 * word/phrase (case-insensitive, longest first) with its neighbouring comma,
 * then tidies spacing and the first letter. This mirrors the intent of the
 * `light` cleanup (T-035) for illustration only; the pipeline in the backend
 * stays the source of truth.
 */
export function previewCrutchRemoval(
  text: string,
  crutches: readonly string[],
): string {
  const words = crutches
    .map(normalizeTerm)
    .filter((word) => word !== "")
    .sort((a, b) => b.length - a.length)
    .map(escapeRegExp);
  if (words.length === 0) return text;
  const pattern = new RegExp(
    `(?<![\\p{L}\\p{N}])(?:${words.join("|")})(?![\\p{L}\\p{N}])\\s*,?`,
    "giu",
  );
  const cleaned = text
    .replace(pattern, " ")
    .replace(/\s+([,.;:!?])/g, "$1")
    .replace(/,\s*,+/g, ",")
    .replace(/^[\s,;]+/, "")
    .replace(/\s{2,}/g, " ")
    .trim();
  return cleaned === ""
    ? cleaned
    : cleaned.charAt(0).toLocaleUpperCase() + cleaned.slice(1);
}

export interface TextSegment {
  text: string;
  /** True when the segment is one of the highlighted terms. */
  hit: boolean;
}

/** Split `text` so the vocabulary terms it contains can be highlighted. */
export function highlightTerms(
  text: string,
  terms: readonly string[],
): TextSegment[] {
  const words = terms
    .map(normalizeTerm)
    .filter((term) => term !== "")
    .sort((a, b) => b.length - a.length)
    .map(escapeRegExp);
  if (words.length === 0 || text === "") return [{ text, hit: false }];
  const pattern = new RegExp(
    `(?<![\\p{L}\\p{N}])(${words.join("|")})(?![\\p{L}\\p{N}])`,
    "giu",
  );
  const segments: TextSegment[] = [];
  let last = 0;
  for (const match of text.matchAll(pattern)) {
    const start = match.index ?? 0;
    if (start > last)
      segments.push({ text: text.slice(last, start), hit: false });
    segments.push({ text: match[0], hit: true });
    last = start + match[0].length;
  }
  if (last < text.length) segments.push({ text: text.slice(last), hit: false });
  return segments.length > 0 ? segments : [{ text, hit: false }];
}
