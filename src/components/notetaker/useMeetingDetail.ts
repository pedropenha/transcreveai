import { useEffect, useState } from "react";
import { commands, type MeetingDetail } from "@/bindings";

export type DetailState =
  | { phase: "loading" }
  | { phase: "missing" }
  | { phase: "error" }
  | { phase: "ready"; detail: MeetingDetail };

/**
 * Loads one meeting for the side panel. `version` changes whenever the list
 * row did (status, title, summary), which re-hydrates the panel; a stale
 * response for a previously selected meeting is dropped.
 */
export function useMeetingDetail(
  id: string | null,
  version: string,
): DetailState {
  const [state, setState] = useState<DetailState>({ phase: "loading" });

  useEffect(() => {
    if (id === null) return;
    let cancelled = false;
    setState((current) =>
      current.phase === "ready" && current.detail.meeting.id === id
        ? current
        : { phase: "loading" },
    );
    commands
      .meetingGet(id)
      .then((result) => {
        if (cancelled) return;
        if (result.status !== "ok") {
          setState({ phase: "error" });
        } else if (result.data === null) {
          setState({ phase: "missing" });
        } else {
          setState({ phase: "ready", detail: result.data });
        }
      })
      .catch((e) => {
        console.warn("meeting_get invoke failed:", e);
        if (!cancelled) setState({ phase: "error" });
      });
    return () => {
      cancelled = true;
    };
  }, [id, version]);

  return state;
}
