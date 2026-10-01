import React, {
  useCallback,
  useDeferredValue,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  Check,
  ClipboardPaste,
  Copy,
  Flag,
  FolderOpen,
  RotateCcw,
  Search,
  Trash2,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import {
  commands,
  events,
  type HistoryEntry,
  type HistoryStatistics,
  type ModelInfo,
} from "@/bindings";
import { useSettings } from "@/hooks/useSettings";
import { Button } from "../../ui/Button";
import { copyToClipboard } from "./clipboard";
import {
  buildWordDiff,
  groupHistoryEntries,
  moveHistorySelection,
  type HistoryEntryView,
} from "./historyView";

const PAGE_SIZE = 80;

type Filters = {
  app: string;
  mode: string;
  status: string;
  period: string;
};

type TimelineRow =
  | { key: string; kind: "heading"; label: string }
  | { key: string; kind: "entry"; entry: HistoryEntry };

const EMPTY_FILTERS: Filters = { app: "", mode: "", status: "", period: "" };

function asHistoryEntryView(entry: HistoryEntry): HistoryEntryView {
  return {
    id: entry.id,
    timestamp: entry.timestamp,
    appName: entry.app_name,
    appExe: entry.app_exe,
    mode: entry.mode,
    status: entry.status,
    rawText: entry.raw_text,
    finalText: entry.final_text,
  };
}

function fromTimestamp(period: string): number | null {
  const days =
    period === "day" ? 1 : period === "week" ? 7 : period === "month" ? 30 : 0;
  return days === 0 ? null : Math.floor(Date.now() / 1000) - days * 86_400;
}

function parseLatency(value: string): Record<string, string | number> {
  try {
    const parsed: unknown = JSON.parse(value);
    return parsed !== null && typeof parsed === "object"
      ? (parsed as Record<string, string | number>)
      : {};
  } catch {
    return {};
  }
}

function formatDuration(seconds: number, locale: string): string {
  const unit = seconds < 60 ? "second" : "minute";
  const value = seconds < 60 ? seconds : seconds / 60;
  return new Intl.NumberFormat(locale, {
    style: "unit",
    unit,
    unitDisplay: "short",
    maximumFractionDigits: 0,
  }).format(value);
}

export const HistorySettings: React.FC = () => {
  const { t, i18n } = useTranslation();
  const { getSetting, updateSetting } = useSettings();
  const [entries, setEntries] = useState<HistoryEntry[]>([]);
  const [stats, setStats] = useState<HistoryStatistics | null>(null);
  const [apps, setApps] = useState<string[]>([]);
  const [query, setQuery] = useState("");
  const deferredQuery = useDeferredValue(query.trim());
  const [filters, setFilters] = useState<Filters>(EMPTY_FILTERS);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [hasMore, setHasMore] = useState(false);
  const [loadError, setLoadError] = useState(false);
  const [retrying, setRetrying] = useState<number | null>(null);
  const [retryModels, setRetryModels] = useState<ModelInfo[]>([]);
  const [retryModelId, setRetryModelId] = useState("");
  const viewportRef = useRef<HTMLDivElement>(null);
  const detailRef = useRef<HTMLElement>(null);
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
        setSelectedId((current) => {
          if (cursor !== null) return current;
          return current !== null &&
            result.data.entries.some((entry) => entry.id === current)
            ? current
            : (result.data.entries[0]?.id ?? null);
        });
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
    const [statsResult, filtersResult] = await Promise.all([
      commands.getHistoryStatistics(),
      commands.getHistoryFilterOptions(),
    ]);
    if (statsResult.status === "ok") setStats(statsResult.data);
    if (filtersResult.status === "ok") setApps(filtersResult.data.apps);
  }, []);

  useEffect(() => {
    setEntries([]);
    setSelectedId(null);
    setHasMore(false);
    viewportRef.current?.scrollTo({ top: 0 });
    void load();
  }, [load]);

  useEffect(() => {
    void refreshSummary();
  }, [refreshSummary]);

  useEffect(() => {
    void Promise.all([
      commands.getAvailableModels(),
      commands.getEffectiveSttModels(),
    ]).then(([modelsResult, effectiveResult]) => {
      if (modelsResult.status === "ok") {
        setRetryModels(
          modelsResult.data.filter((model) => model.is_downloaded),
        );
      }
      if (effectiveResult.status === "ok") {
        setRetryModelId(effectiveResult.data.dictation ?? "");
      }
    });
  }, []);

  useEffect(() => {
    const unlisten = events.historyUpdatePayload.listen(() => {
      void load();
      void refreshSummary();
    });
    return () => void unlisten.then((stop) => stop());
  }, [load, refreshSummary]);

  const rows = useMemo<TimelineRow[]>(() => {
    const groups = groupHistoryEntries(
      entries.map(asHistoryEntryView),
      i18n.language,
    );
    const byId = new Map(entries.map((entry) => [entry.id, entry]));
    return groups.flatMap((group) => [
      {
        key: `heading-${group.key}`,
        kind: "heading" as const,
        label: group.label,
      },
      ...group.entries.map((view) => ({
        key: `entry-${view.id}`,
        kind: "entry" as const,
        entry: byId.get(view.id)!,
      })),
    ]);
  }, [entries, i18n.language]);

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => viewportRef.current,
    estimateSize: (index) => (rows[index]?.kind === "heading" ? 42 : 86),
    overscan: 8,
    getItemKey: (index) => rows[index]?.key ?? index,
  });
  const virtualItems = virtualizer.getVirtualItems();

  useEffect(() => {
    const last = virtualItems[virtualItems.length - 1];
    if (
      last &&
      last.index >= rows.length - 6 &&
      hasMore &&
      !loading &&
      !loadingMore &&
      !loadError
    ) {
      const cursor = entries[entries.length - 1]?.id;
      if (cursor !== undefined) void load(cursor);
    }
  }, [
    entries,
    hasMore,
    load,
    loadError,
    loading,
    loadingMore,
    rows.length,
    virtualItems,
  ]);

  const selected = entries.find((entry) => entry.id === selectedId) ?? null;
  const selectEntry = (id: number) => {
    setSelectedId(id);
    const index = rows.findIndex(
      (row) => row.kind === "entry" && row.entry.id === id,
    );
    if (index >= 0) virtualizer.scrollToIndex(index, { align: "auto" });
  };

  const handleKeyboard = async (event: React.KeyboardEvent<HTMLDivElement>) => {
    const ids = entries.map((entry) => entry.id);
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const next = moveHistorySelection(
        ids,
        selectedId,
        event.key === "ArrowDown" ? "next" : "previous",
      );
      if (next !== null) selectEntry(next);
    } else if (event.key === "Enter" && selected) {
      event.preventDefault();
      detailRef.current?.focus();
    } else if (
      (event.ctrlKey || event.metaKey) &&
      event.key.toLowerCase() === "c" &&
      selected
    ) {
      event.preventDefault();
      const copied = await copyToClipboard(selected.final_text);
      copied
        ? toast.success(t("settings.history.copied"))
        : toast.error(t("settings.history.copyError"));
    }
  };

  const copyEntry = async (entry: HistoryEntry) => {
    const copied = await copyToClipboard(entry.final_text);
    copied
      ? toast.success(t("settings.history.copied"))
      : toast.error(t("settings.history.copyError"));
  };

  const deleteEntry = async (entry: HistoryEntry) => {
    if (!window.confirm(t("settings.history.deleteConfirm"))) return;
    const result = await commands.deleteHistoryEntry(entry.id);
    if (result.status !== "ok")
      return toast.error(t("settings.history.deleteError"));
    const index = entries.findIndex((item) => item.id === entry.id);
    const remaining = entries.filter((item) => item.id !== entry.id);
    setEntries(remaining);
    setSelectedId(remaining[Math.min(index, remaining.length - 1)]?.id ?? null);
    requestAnimationFrame(() => viewportRef.current?.focus());
    toast.success(t("settings.history.deleted"));
  };

  const retryEntry = async (entry: HistoryEntry) => {
    setRetrying(entry.id);
    try {
      if (retryModelId) {
        const providerResult = await commands.setSttProvider(
          "dictation",
          `local_model:${retryModelId}`,
        );
        if (providerResult.status !== "ok") {
          toast.error(t("settings.history.retranscribeError"));
          return;
        }
      }
      const result = await commands.retryHistoryEntryTranscription(entry.id);
      if (result.status !== "ok")
        toast.error(t("settings.history.retranscribeError"));
    } catch {
      toast.error(t("settings.history.retranscribeError"));
    } finally {
      setRetrying(null);
    }
  };

  const addSelectionToDictionary = async (input = "") => {
    const term = (input || window.getSelection()?.toString() || "")
      .replace(/\s+/g, " ")
      .trim();
    if (!term) return toast.error(t("settings.history.selectDictionaryText"));
    const words = getSetting("custom_words") ?? [];
    if (!words.includes(term))
      await updateSetting("custom_words", [...words, term]);
    toast.success(t("settings.history.dictionaryAdded", { term }));
  };

  return (
    <div className="hub-history" aria-busy={loading}>
      <main className="hub-history-main">
        <header className="hub-history-header">
          <div>
            <h1>{t("settings.history.title")}</h1>
            <p>{t("settings.history.subtitle")}</p>
          </div>
          <Button
            variant="secondary"
            size="sm"
            onClick={() => void commands.openRecordingsFolder()}
          >
            <FolderOpen aria-hidden="true" className="h-4 w-4" />
            {t("settings.history.openFolder")}
          </Button>
        </header>

        <section
          className="history-stats"
          aria-label={t("settings.history.statistics.label")}
        >
          {[
            [stats?.words_today ?? 0, t("settings.history.statistics.today")],
            [stats?.words_week ?? 0, t("settings.history.statistics.week")],
            [stats?.words_total ?? 0, t("settings.history.statistics.total")],
            [
              Math.round(stats?.words_per_minute ?? 0),
              t("settings.history.statistics.wpm"),
            ],
            [
              formatDuration(stats?.seconds_saved ?? 0, i18n.language),
              t("settings.history.statistics.saved"),
            ],
          ].map(([value, label]) => (
            <div className="history-stat" key={label}>
              <strong>
                {typeof value === "number"
                  ? new Intl.NumberFormat(i18n.language).format(value)
                  : value}
              </strong>
              <span>{label}</span>
            </div>
          ))}
          {stats?.provider_usage.map((usage) => (
            <div className="history-stat" key={usage.provider_id}>
              <strong>
                {formatDuration(usage.duration_ms / 1000, i18n.language)}
              </strong>
              <span>
                {t("settings.history.statistics.providerUsage", {
                  provider:
                    usage.provider_id === "local"
                      ? t("settings.history.localProvider")
                      : usage.provider_id,
                  cost: new Intl.NumberFormat(i18n.language, {
                    style: "currency",
                    currency: "USD",
                  }).format(usage.estimated_cost),
                })}
              </span>
            </div>
          ))}
        </section>

        <div className="history-toolbar">
          <label className="history-search">
            <span className="sr-only">{t("settings.history.searchLabel")}</span>
            <Search aria-hidden="true" className="h-4 w-4" />
            <input
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder={t("settings.history.searchPlaceholder")}
              type="search"
            />
          </label>
          {(["app", "mode", "status", "period"] as const).map((name) => (
            <label key={name}>
              <span className="sr-only">
                {t(`settings.history.filters.${name}`)}
              </span>
              <select
                value={filters[name]}
                onChange={(event) =>
                  setFilters((current) => ({
                    ...current,
                    [name]: event.target.value,
                  }))
                }
              >
                <option value="">
                  {t(`settings.history.filters.${name}`)}
                </option>
                {name === "app" &&
                  apps.map((app) => (
                    <option key={app} value={app}>
                      {app}
                    </option>
                  ))}
                {name === "mode" &&
                  ["dictation", "command", "note"].map((value) => (
                    <option key={value} value={value}>
                      {t(`settings.history.modes.${value}`)}
                    </option>
                  ))}
                {name === "status" &&
                  [
                    "inserted",
                    "copied",
                    "failed",
                    "cancelled",
                    "saved_note",
                  ].map((value) => (
                    <option key={value} value={value}>
                      {t(`settings.history.status.${value}`)}
                    </option>
                  ))}
                {name === "period" &&
                  ["day", "week", "month"].map((value) => (
                    <option key={value} value={value}>
                      {t(`settings.history.periods.${value}`)}
                    </option>
                  ))}
              </select>
            </label>
          ))}
        </div>

        <div
          ref={viewportRef}
          className="history-viewport"
          role="region"
          aria-label={t("settings.history.listLabel")}
          aria-busy={loading || loadingMore}
          tabIndex={0}
          onKeyDown={handleKeyboard}
        >
          <p className="sr-only" role="status" aria-live="polite">
            {loading
              ? t("settings.history.loading")
              : t("settings.history.resultCount", { count: entries.length })}
          </p>
          {loading ? (
            <p className="history-empty">{t("settings.history.loading")}</p>
          ) : null}
          {!loading && rows.length === 0 ? (
            <p className="history-empty">{t("settings.history.empty")}</p>
          ) : null}
          {loadError ? (
            <p className="history-empty" role="alert">
              {t("settings.history.loadError")}
            </p>
          ) : null}
          <div
            className="history-virtual-space"
            style={{ height: virtualizer.getTotalSize() }}
          >
            {virtualItems.map((virtualRow) => {
              const row = rows[virtualRow.index];
              return (
                <div
                  key={row.key}
                  ref={virtualizer.measureElement}
                  data-index={virtualRow.index}
                  className="history-virtual-row"
                  style={{ transform: `translateY(${virtualRow.start}px)` }}
                >
                  {row.kind === "heading" ? (
                    <h2 className="history-day">{row.label}</h2>
                  ) : (
                    <button
                      id={`history-entry-${row.entry.id}`}
                      type="button"
                      data-selected={selectedId === row.entry.id}
                      className="history-row"
                      onClick={() => selectEntry(row.entry.id)}
                    >
                      <span
                        className={`history-node history-node-${row.entry.mode}`}
                        aria-hidden="true"
                      />
                      <time
                        dateTime={new Date(
                          row.entry.timestamp * 1000,
                        ).toISOString()}
                      >
                        {new Date(
                          row.entry.timestamp * 1000,
                        ).toLocaleTimeString(i18n.language, {
                          hour: "2-digit",
                          minute: "2-digit",
                        })}
                      </time>
                      <span className="history-copy">
                        <strong>
                          {row.entry.app_name ||
                            row.entry.app_exe ||
                            t("settings.history.unknownApp")}{" "}
                          · {t(`settings.history.modes.${row.entry.mode}`)}
                        </strong>
                        <span>
                          {row.entry.final_text ||
                            t("settings.history.transcriptionFailed")}
                        </span>
                      </span>
                      <span
                        className={`history-status history-status-${row.entry.status}`}
                      >
                        {t(`settings.history.status.${row.entry.status}`)}
                      </span>
                    </button>
                  )}
                </div>
              );
            })}
          </div>
          {loadingMore ? (
            <p className="history-loading-more">
              {t("settings.history.loadingMore")}
            </p>
          ) : null}
        </div>
      </main>

      <aside
        ref={detailRef}
        className="hub-history-detail"
        tabIndex={-1}
        aria-label={t("settings.history.detail.label")}
      >
        {selected ? (
          <HistoryDetail
            entry={selected}
            retrying={retrying === selected.id}
            retryModels={retryModels}
            retryModelId={retryModelId}
            onRetryModelChange={setRetryModelId}
            onCopy={() => void copyEntry(selected)}
            onReinsert={async () => {
              const result = await commands.reinsertHistoryEntry(selected.id);
              result.status === "ok"
                ? toast.success(t("settings.history.reinserted"))
                : toast.error(t("settings.history.reinsertError"));
            }}
            onRetry={() => void retryEntry(selected)}
            onDelete={() => void deleteEntry(selected)}
            onToggleFlag={() =>
              void commands.toggleHistoryEntrySaved(selected.id)
            }
            onAddDictionary={(term) => void addSelectionToDictionary(term)}
          />
        ) : (
          <p className="history-empty">{t("settings.history.detail.empty")}</p>
        )}
      </aside>
    </div>
  );
};

interface HistoryDetailProps {
  entry: HistoryEntry;
  retrying: boolean;
  retryModels: ModelInfo[];
  retryModelId: string;
  onRetryModelChange: (modelId: string) => void;
  onCopy: () => void;
  onReinsert: () => void;
  onRetry: () => void;
  onDelete: () => void;
  onToggleFlag: () => void;
  onAddDictionary: (term: string) => void;
}

const HistoryDetail: React.FC<HistoryDetailProps> = ({
  entry,
  retrying,
  retryModels,
  retryModelId,
  onRetryModelChange,
  onCopy,
  onReinsert,
  onRetry,
  onDelete,
  onToggleFlag,
  onAddDictionary,
}) => {
  const { t, i18n } = useTranslation();
  const [dictionaryTerm, setDictionaryTerm] = useState("");
  const latency = useMemo(
    () => parseLatency(entry.latency_json),
    [entry.latency_json],
  );
  const diff = useMemo(
    () => buildWordDiff(entry.raw_text, entry.final_text),
    [entry.final_text, entry.raw_text],
  );
  const latencyLabel = (value: string | number | undefined) =>
    value === undefined
      ? t("settings.history.detail.unavailable")
      : t("settings.history.detail.milliseconds", { value });
  return (
    <>
      <p className="detail-label">{t("settings.history.detail.selected")}</p>
      <h2>
        {entry.app_name || entry.app_exe || t("settings.history.unknownApp")}
      </h2>
      <p className="detail-meta">
        {new Date(entry.timestamp * 1000).toLocaleString(i18n.language)} ·{" "}
        {t(`settings.history.status.${entry.status}`)}
      </p>
      <label className="detail-retry-model">
        <span>{t("settings.history.retryModel")}</span>
        <select
          value={retryModelId}
          onChange={(event) => onRetryModelChange(event.target.value)}
          disabled={retrying || retryModels.length === 0}
        >
          {retryModels.map((model) => (
            <option key={model.id} value={model.id}>
              {model.name}
            </option>
          ))}
        </select>
      </label>
      <div className="detail-actions">
        <button
          type="button"
          onClick={onCopy}
          aria-label={t("settings.history.copyToClipboard")}
          title={t("settings.history.copyToClipboard")}
        >
          <Copy aria-hidden="true" />
        </button>
        <button
          type="button"
          onClick={onReinsert}
          aria-label={t("settings.history.reinsert")}
          title={t("settings.history.reinsert")}
        >
          <ClipboardPaste aria-hidden="true" />
        </button>
        <button
          type="button"
          onClick={onToggleFlag}
          aria-label={t("settings.history.flag")}
          aria-pressed={entry.saved}
          title={t("settings.history.flag")}
        >
          <Flag
            aria-hidden="true"
            fill={entry.saved ? "currentColor" : "none"}
          />
        </button>
        <button
          type="button"
          onClick={onRetry}
          disabled={!entry.audio_available || retrying}
          aria-label={t("settings.history.retranscribe")}
          title={t("settings.history.retranscribe")}
        >
          <RotateCcw
            aria-hidden="true"
            className={retrying ? "animate-spin" : ""}
          />
        </button>
        <button
          type="button"
          onClick={onDelete}
          aria-label={t("settings.history.delete")}
          title={t("settings.history.delete")}
        >
          <Trash2 aria-hidden="true" />
        </button>
      </div>
      <p className="detail-label">{t("settings.history.detail.final")}</p>
      <p className="detail-copy select-text">{entry.final_text}</p>
      <form
        className="detail-dictionary"
        onSubmit={(event) => {
          event.preventDefault();
          onAddDictionary(dictionaryTerm);
          setDictionaryTerm("");
        }}
      >
        <label htmlFor={`history-dictionary-${entry.id}`} className="sr-only">
          {t("settings.history.dictionaryTermLabel")}
        </label>
        <input
          id={`history-dictionary-${entry.id}`}
          value={dictionaryTerm}
          onChange={(event) => setDictionaryTerm(event.target.value)}
          placeholder={t("settings.history.dictionaryTermPlaceholder")}
        />
        <button type="submit">{t("settings.history.addDictionary")}</button>
      </form>
      <p className="detail-label">{t("settings.history.detail.diff")}</p>
      <p className="detail-diff select-text">
        {diff.map((part, index) => (
          <span key={`${part.kind}-${index}`} className={`diff-${part.kind}`}>
            {part.kind !== "equal" ? (
              <span className="sr-only">
                {t(`settings.history.detail.${part.kind}`)}:{" "}
              </span>
            ) : null}
            {part.kind === "added" ? <Check aria-hidden="true" /> : null}
            {part.text}{" "}
          </span>
        ))}
      </p>
      <p className="detail-label">{t("settings.history.detail.pipeline")}</p>
      <dl className="detail-grid">
        <div>
          <dt>{t("settings.history.detail.provider")}</dt>
          <dd>
            {retryModels.find((model) =>
              entry.stt_provider_id?.endsWith(model.id),
            )?.name ||
              entry.stt_provider_id ||
              t("settings.history.localProvider")}
          </dd>
        </div>
        <div>
          <dt>{t("settings.history.detail.language")}</dt>
          <dd>{entry.language || t("settings.history.detail.unavailable")}</dd>
        </div>
        <div>
          <dt>STT</dt>
          <dd>{latencyLabel(latency.stt_ms)}</dd>
        </div>
        <div>
          <dt>LLM</dt>
          <dd>{latencyLabel(latency.llm_ms)}</dd>
        </div>
        <div>
          <dt>{t("settings.history.detail.insertion")}</dt>
          <dd>{latencyLabel(latency.insert_ms)}</dd>
        </div>
        <div>
          <dt>{t("settings.history.detail.duration")}</dt>
          <dd>{formatDuration(entry.duration_ms / 1000, i18n.language)}</dd>
        </div>
      </dl>
    </>
  );
};
