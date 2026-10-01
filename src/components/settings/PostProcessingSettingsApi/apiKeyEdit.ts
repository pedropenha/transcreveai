/**
 * Decide the new field value after an edit while a masked vault hint
 * ("••••1234") may be displayed.
 *
 * The bullet character can only come from the mask itself — API keys never
 * contain it. A value that still contains a bullet is therefore a mangled
 * hint (e.g. a mid-mask backspace producing "••••123"), never new key
 * material, so the edit is discarded and the hint restored. Only wholly new
 * text (the focus handler selects the whole mask, so typing replaces it) or
 * a cleared field is accepted.
 */
export function apiKeyEditValue(hint: string, next: string): string {
  return next.includes("•") ? hint : next;
}
