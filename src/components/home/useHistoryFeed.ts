import {
  useCallback,
  useDeferredValue,
  useEffect,
  useRef,
  useState,
} from "react";
import {
  commands,
  events,
  type HistoryEntry,
  type HistoryStatistics,
} from "@/bindings";
import { isStreakExact } from "./homeView";

const PAGE_SIZE = 80;
const STREAK_PAGE_SIZE = 100;
const STREAK_MAX_PAGES = 5;

export interface HistoryFilters {
  app: string;
  mode: string;
  status: string;
  period: string;
}

export const EMPTY_FILTERS: HistoryFilters = {
  app: "",
  mode: "",
  status: "",
  period: "",
};

function fromTimestamp(period: string): number | null {
  const days =
    period === "day" ? 1 : period === "week" ? 7 : period === "month" ? 30 : 0;
  return days === 0 ? null : Math.floor(Date.now() / 1000) - days * 86_400;
}

/**
 * Timestamps of the most recent dictations, unfiltered, loaded page by page
 * until the activity streak provably cannot grow (or a page cap is hit), so the
 * streak never depends on the search/filter currently shown.
 */
async function loadStreakTimestamps(): Promise<number[]> {
  const timestamps: number[] = [];
  let cursor: number | null = null;
  for (let page = 0; page < STREAK_MAX_PAGES; page += 1) {
    const result = await commands.searchHistoryEntries({
      search: null,
      app: null,
      mode: null,
      status: null,
      from_timestamp: null,
      to_timestamp: null,
      cursor,
      limit: STREAK_PAGE_SIZE,
    });
    if (result.status !== "ok") break;
    const { entries, has_more } = result.data;
    timestamps.push(...entries.map((entry) => entry.timestamp));
    if (!has_more || entries.length === 0) break;
    if (isStreakExact(timestamps, new Date())) break;
    cursor = entries[entries.length - 1].id;
  }
  return timestamps;
}

/**
 * Paginated, filterable dictation history plus the summary (stats, app
 * filter options). Cursor pagination; stale responses are discarded by a
 * request counter. Refreshes when the backend reports a history update.
 */
export function useHistoryFeed() {
  const [entries, setEntries] = useState<HistoryEntry[]>([]);
  const [stats, setStats] = useState<HistoryStatistics | null>(null);
  const [apps, setApps] = useState<string[]>([]);
  const [streakTimestamps, setStreakTimestamps] = useState<number[]>([]);
  const [query, setQuery] = useState("");
  const deferredQuery = useDeferredValue(query.trim());
  const [filters, setFilters] = useState<HistoryFilters>(EMPTY_FILTERS);
  const [loading, setLoading] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [hasMore, setHasMore] = useState(false);
  const [loadError, setLoadError] = useState(false);
  const requestRef = useRef(0);

  const load = useCallback(
    async (cursor: number | null = null) => {
      const request = ++requestRef.current;
      if (cursor === null) setLoading(true);
      else setLoadingMore(true);
      setLoadError(false);
      try {
        const result = await commands.searchHistoryEntries({
          search: deferredQuery || null,
          app: filters.app || null,
          mode: filters.mode || null,
          status: filters.status || null,
          from_timestamp: fromTimestamp(filters.period),
          to_timestamp: null,
          cursor,
          limit: PAGE_SIZE,
        });
        if (request !== requestRef.current) return;
        if (result.status !== "ok") {
          setLoadError(true);
          setHasMore(false);
          return;
        }
        setEntries((current) => {
          const combined =
            cursor === null
              ? result.data.entries
              : [...current, ...result.data.entries];
          return [
            ...new Map(combined.map((entry) => [entry.id, entry])).values(),
          ];
        });
        setHasMore(result.data.has_more);
      } catch {
        if (request === requestRef.current) {
          setLoadError(true);
          setHasMore(false);
        }
      } finally {
        if (request === requestRef.current) {
          setLoading(false);
          setLoadingMore(false);
        }
      }
    },
    [deferredQuery, filters],
  );

  const refreshSummary = useCallback(async () => {
    const [statsResult, filtersResult, streak] = await Promise.all([
      commands.getHistoryStatistics(),
      commands.getHistoryFilterOptions(),
      loadStreakTimestamps().catch(() => [] as number[]),
    ]);
    setStreakTimestamps(streak);
    if (statsResult.status === "ok") setStats(statsResult.data);
    if (filtersResult.status === "ok") setApps(filtersResult.data.apps);
  }, []);

  useEffect(() => {
    setEntries([]);
    setHasMore(false);
    void load();
  }, [load]);

  useEffect(() => {
    void refreshSummary();
  }, [refreshSummary]);

  useEffect(() => {
    const unlisten = events.historyUpdatePayload.listen(() => {
      void load();
      void refreshSummary();
    });
    return () => void unlisten.then((stop) => stop());
  }, [load, refreshSummary]);

  const loadMore = useCallback(() => {
    const cursor = entries[entries.length - 1]?.id;
    if (cursor !== undefined) void load(cursor);
  }, [entries, load]);

  const removeEntry = useCallback((id: number) => {
    setEntries((current) => current.filter((entry) => entry.id !== id));
  }, []);

  const removeEntries = useCallback((ids: readonly number[]) => {
    const drop = new Set(ids);
    setEntries((current) => current.filter((entry) => !drop.has(entry.id)));
  }, []);

  return {
    entries,
    stats,
    apps,
    streakTimestamps,
    query,
    setQuery,
    filters,
    setFilters,
    loading,
    loadingMore,
    hasMore,
    loadError,
    loadMore,
    removeEntry,
    removeEntries,
  };
}
