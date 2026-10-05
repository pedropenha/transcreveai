import { commands } from "@/bindings";

/**
 * Per-entry cache of origin-app icons extracted by the backend
 * (`history_app_icon`). `null` is a cached "no icon" so the same entry is never
 * asked twice while cached; in-flight lookups are shared between rows. Keep
 * at most 128 answers so browsing a long history does not retain every PNG.
 */
const CACHE_CAPACITY = 128;
const resolved = new Map<string, string | null>();
const inflight = new Map<string, Promise<string | null>>();

export function cachedOriginIcon(key: string): string | null | undefined {
  return resolved.get(key);
}

export function loadOriginIcon(
  key: string,
  entryId: number,
): Promise<string | null> {
  const known = resolved.get(key);
  if (known !== undefined) return Promise.resolve(known);
  const pending = inflight.get(key);
  if (pending) return pending;

  const request = commands
    .historyAppIcon(entryId)
    .then((result) => {
      // Only a definitive answer is cached; failures may succeed later.
      if (result.status === "ok") {
        if (resolved.size >= CACHE_CAPACITY) {
          resolved.delete(resolved.keys().next().value!);
        }
        resolved.set(key, result.data);
      }
      return result.status === "ok" ? result.data : null;
    })
    .catch((e) => {
      console.warn("history_app_icon invoke failed:", e);
      return null;
    })
    .finally(() => inflight.delete(key));
  inflight.set(key, request);
  return request;
}
