import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  commands,
  type MeetingListItem,
  type MeetingStateEvent,
} from "@/bindings";
import {
  applyProgress,
  pruneProgress,
  type ProgressMap,
} from "./notetakerView";

const STATE_EVENT = "meeting://state";
const PROGRESS_EVENT = "meeting://progress";

interface ProgressPayload {
  meeting_id: string;
  step?: string;
  pct?: number;
}

export interface MeetingFeed {
  items: MeetingListItem[];
  loading: boolean;
  /** Last `meeting://progress` percentage per processing meeting. */
  progress: ProgressMap;
  /** Latest `meeting://state` snapshot (elapsed time of the live meeting). */
  liveState: MeetingStateEvent | null;
  refresh: () => Promise<void>;
  /** Optimistic removal (delete); in-flight search responses are dropped. */
  removeLocal: (id: string) => void;
}

/**
 * Meetings for the Notetaker list. The backend owns the lifecycle: it persists
 * a status change, then emits `meeting://state`, so the event is the refresh
 * trigger. `meeting://progress` only feeds the "Transcrevendo %" chip.
 */
export function useMeetingFeed(query: string): MeetingFeed {
  const [items, setItems] = useState<MeetingListItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [progress, setProgress] = useState<ProgressMap>(new Map());
  const [liveState, setLiveState] = useState<MeetingStateEvent | null>(null);
  const seq = useRef(0);
  const lastLifecycle = useRef<string | null>(null);
  // Ids deleted optimistically; filtered from any response until it catches up.
  const deleted = useRef<Set<string>>(new Set());

  const refresh = useCallback(async () => {
    const mine = ++seq.current;
    try {
      const result = await commands.meetingSearch(query);
      if (mine !== seq.current) return;
      if (result.status === "ok") {
        const rows = result.data.filter((m) => !deleted.current.has(m.id));
        setItems(rows);
        setProgress((current) => pruneProgress(current, rows));
      } else {
        console.warn("meeting_search failed:", result.error);
      }
    } catch (e) {
      console.warn("meeting_search invoke failed:", e);
    }
  }, [query]);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    void refresh().finally(() => {
      if (!cancelled) setLoading(false);
    });
    return () => {
      cancelled = true;
    };
  }, [refresh]);

  // Catch up with a recording that started before this screen mounted.
  useEffect(() => {
    let cancelled = false;
    void commands
      .meetingCurrent()
      .then((result) => {
        if (!cancelled && result.status === "ok") {
          setLiveState((current) => current ?? result.data);
        }
      })
      .catch((e) => console.warn("meeting_current invoke failed:", e));
    return () => {
      cancelled = true;
    };
  }, []);

  // Subscribe once; the latest `refresh` is read through a ref so a changing
  // query never opens a window in which events are missed.
  const refreshRef = useRef(refresh);
  useEffect(() => {
    refreshRef.current = refresh;
  }, [refresh]);

  useEffect(() => {
    let disposed = false;
    const unlisteners: Array<() => void> = [];
    const track = (promise: Promise<() => void>) => {
      void promise
        .then((fn) => {
          if (disposed) fn();
          else unlisteners.push(fn);
        })
        .catch((e) => console.warn("meeting listen failed:", e));
    };
    track(
      listen<MeetingStateEvent>(STATE_EVENT, (event) => {
        setLiveState(event.payload);
        const key = `${event.payload.meeting_id}:${event.payload.status}`;
        if (key === lastLifecycle.current) return;
        lastLifecycle.current = key;
        void refreshRef.current();
      }),
    );
    track(
      listen<ProgressPayload>(PROGRESS_EVENT, (event) => {
        setProgress((current) => applyProgress(current, event.payload));
      }),
    );
    return () => {
      disposed = true;
      unlisteners.forEach((fn) => fn());
    };
  }, []);

  const removeLocal = useCallback((id: string) => {
    deleted.current.add(id);
    setItems((current) => current.filter((m) => m.id !== id));
  }, []);

  return { items, loading, progress, liveState, refresh, removeLocal };
}
