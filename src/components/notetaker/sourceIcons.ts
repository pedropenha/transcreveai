import { commands } from "@/bindings";

/**
 * Per-executable cache of icons extracted by the backend
 * (`meeting_source_icon`). `null` is a cached "no icon" so the same exe is
 * never asked twice; in-flight lookups are shared between rows.
 */
const resolved = new Map<string, string | null>();
const inflight = new Map<string, Promise<string | null>>();

export function cachedSourceIcon(key: string): string | null | undefined {
  return resolved.get(key);
}

export function loadSourceIcon(
  key: string,
  meetingId: string,
): Promise<string | null> {
  const known = resolved.get(key);
  if (known !== undefined) return Promise.resolve(known);
  const pending = inflight.get(key);
  if (pending) return pending;

  const request = commands
    .meetingSourceIcon(meetingId)
    .then((result) => {
      // Only a definitive answer is cached; failures may succeed later.
      if (result.status === "ok") resolved.set(key, result.data);
      return result.status === "ok" ? result.data : null;
    })
    .catch((e) => {
      console.warn("meeting_source_icon invoke failed:", e);
      return null;
    })
    .finally(() => inflight.delete(key));
  inflight.set(key, request);
  return request;
}

/** Test seam: forget everything. */
export function resetSourceIconCache(): void {
  resolved.clear();
  inflight.clear();
}
