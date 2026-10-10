import React, {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  FolderOpen,
  Keyboard,
  ListChecks,
  Search,
  Trash2,
  X,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { commands, type HistoryEntry } from "@/bindings";
import { useDismissedUi } from "@/hooks/useDismissedUi";
import { useNow } from "@/hooks/useNow";
import { useModelStore } from "@/stores/modelStore";
import { useHubNavigation } from "../shell/HubNavigation";
import { moveHistorySelection } from "../settings/history/historyView";
import { HistoryDetail } from "./HistoryDetail";
import { HistoryRow } from "./HistoryRow";
import { HomeBanner } from "./HomeBanner";
import { StatsCard } from "./StatsCard";
import {
  applySelectAll,
  computeStreak,
  greetingPeriod,
  groupEntriesByDay,
  selectAllState,
  selectedLoadedIds,
  toggleSelected,
  type DayKind,
} from "./homeView";
import { useHistoryActions } from "./useHistoryActions";
import { useHistoryFeed } from "./useHistoryFeed";
import "./home.css";

const BANNER_ID = "home_banner";
const TIP_ID = "home_tip";
const HEADING_HEIGHT = 44;
const ROW_HEIGHT = 112;
const LOAD_MORE_THRESHOLD = 6;

type Position = "first" | "middle" | "last" | "only";

type HomeRow =
  | { key: string; kind: "heading"; dayKind: DayKind; date: Date }
  | { key: string; kind: "entry"; entry: HistoryEntry; position: Position };

function positionOf(index: number, size: number): Position {
  if (size === 1) return "only";
  if (index === 0) return "first";
  return index === size - 1 ? "last" : "middle";
}

/** Distance from the top of the scroll container to the list, kept fresh. */
function useScrollMargin(
  scrollRef: React.RefObject<HTMLElement | null>,
  listRef: React.RefObject<HTMLElement | null>,
  layoutKey: string,
): number {
  const [margin, setMargin] = useState(0);
  useLayoutEffect(() => {
    const measure = () => {
      const scroller = scrollRef.current;
      const list = listRef.current;
      if (!scroller || !list) return;
      setMargin(
        list.getBoundingClientRect().top -
          scroller.getBoundingClientRect().top +
          scroller.scrollTop,
      );
    };
    measure();
    const observer = new ResizeObserver(measure);
    if (scrollRef.current) observer.observe(scrollRef.current);
    return () => observer.disconnect();
  }, [scrollRef, listRef, layoutKey]);
  return margin;
}

export const HomePage: React.FC = () => {
  const { t, i18n } = useTranslation();
  const navigate = useHubNavigation();
  const { isDismissed, dismiss } = useDismissedUi();
  const feed = useHistoryFeed();
  const models = useModelStore((state) => state.models);
  const currentModel = useModelStore((state) => state.currentModel);
  const [detailId, setDetailId] = useState<number | null>(null);
  const [activeId, setActiveId] = useState<number | null>(null);
  // T-115 multi-select: a "Selecionar" mode adds per-row checkboxes and a
  // bar with select-all + "Apagar selecionadas (N)".
  const [selecting, setSelecting] = useState(false);
  const [selected, setSelected] = useState<ReadonlySet<number>>(new Set());
  const [deletingSelected, setDeletingSelected] = useState(false);
  const entriesRef = useRef(feed.entries);
  entriesRef.current = feed.entries;

  // After a delete, keep keyboard focus on a neighbouring row (or the list).
  const handleDeleted = useCallback(
    (id: number) => {
      const ids = entriesRef.current.map((entry) => entry.id);
      const index = ids.indexOf(id);
      const neighbour = ids[index + 1] ?? ids[index - 1] ?? null;
      feed.removeEntry(id);
      setDetailId(null);
      setActiveId(neighbour);
      requestAnimationFrame(() =>
        (neighbour !== null
          ? document.getElementById(`history-entry-${neighbour}`)
          : listRef.current
        )?.focus(),
      );
    },
    [feed],
  );
  const actions = useHistoryActions(handleDeleted);
  const scrollRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const drawerRef = useRef<HTMLElement>(null);
  const lastFocusedRef = useRef<number | null>(null);

  const showBanner = !isDismissed(BANNER_ID);
  const showTip = !isDismissed(TIP_ID);
  const now = useNow();
  const dayFormatter = useMemo(
    () =>
      new Intl.DateTimeFormat(i18n.language, {
        weekday: "long",
        day: "numeric",
        month: "long",
      }),
    [i18n.language],
  );

  const rows = useMemo<HomeRow[]>(
    () =>
      groupEntriesByDay(feed.entries, now).flatMap((group) => [
        {
          key: `day-${group.key}`,
          kind: "heading" as const,
          dayKind: group.kind,
          date: group.date,
        },
        ...group.entries.map((entry, index) => ({
          key: `entry-${entry.id}`,
          kind: "entry" as const,
          entry,
          position: positionOf(index, group.entries.length),
        })),
      ]),
    [feed.entries, now],
  );

  const scrollMargin = useScrollMargin(
    scrollRef,
    listRef,
    `${showBanner}-${feed.entries.length === 0}-${feed.loading}-${feed.loadError}`,
  );
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: (index) =>
      rows[index]?.kind === "heading" ? HEADING_HEIGHT : ROW_HEIGHT,
    overscan: 8,
    scrollMargin,
    getItemKey: (index) => rows[index]?.key ?? index,
  });
  const virtualItems = virtualizer.getVirtualItems();

  const { hasMore, loading, loadingMore, loadError, loadMore } = feed;
  useEffect(() => {
    const last = virtualItems[virtualItems.length - 1];
    if (
      last &&
      last.index >= rows.length - LOAD_MORE_THRESHOLD &&
      hasMore &&
      !loading &&
      !loadingMore &&
      !loadError
    ) {
      loadMore();
    }
  }, [
    hasMore,
    loadError,
    loadMore,
    loading,
    loadingMore,
    rows.length,
    virtualItems,
  ]);

  const entryIds = useMemo(
    () => feed.entries.map((entry) => entry.id),
    [feed.entries],
  );
  const selectedIds = useMemo(
    () => selectedLoadedIds(entryIds, selected),
    [entryIds, selected],
  );
  const allState = selectAllState(entryIds, selected);

  const toggleSelect = useCallback((id: number) => {
    setSelected((current) => toggleSelected(current, id));
  }, []);

  const exitSelection = useCallback(() => {
    setSelecting(false);
    setSelected(new Set());
  }, []);

  const deleteSelected = useCallback(async () => {
    const ids = selectedIds;
    if (ids.length === 0) return;
    const confirmed = window.confirm(
      t("settings.history.deleteSelectedConfirm", { count: ids.length }),
    );
    if (!confirmed) return;
    setDeletingSelected(true);
    try {
      const result = await commands.deleteHistoryEntries(ids);
      if (result.status !== "ok") {
        toast.error(t("settings.history.deleteError"));
        return;
      }
      feed.removeEntries(ids);
      exitSelection();
      setDetailId((current) =>
        current !== null && ids.includes(current) ? null : current,
      );
      toast.success(
        t("settings.history.deletedSelected", { count: result.data }),
      );
    } catch {
      toast.error(t("settings.history.deleteError"));
    } finally {
      setDeletingSelected(false);
    }
  }, [exitSelection, feed, selectedIds, t]);

  const focusEntry = useCallback(
    (id: number) => {
      const index = rows.findIndex(
        (row) => row.kind === "entry" && row.entry.id === id,
      );
      if (index >= 0) virtualizer.scrollToIndex(index, { align: "auto" });
      requestAnimationFrame(() =>
        document.getElementById(`history-entry-${id}`)?.focus(),
      );
    },
    [rows, virtualizer],
  );

  const handleRowKeyDown = (
    event: React.KeyboardEvent<HTMLElement>,
    entry: HistoryEntry,
  ) => {
    if (event.target !== event.currentTarget) return;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const next = moveHistorySelection(
        feed.entries.map((item) => item.id),
        entry.id,
        event.key === "ArrowDown" ? "next" : "previous",
      );
      if (next !== null && next !== entry.id) focusEntry(next);
    } else if (event.key === "Home" || event.key === "End") {
      event.preventDefault();
      const ids = feed.entries.map((item) => item.id);
      const edge = event.key === "Home" ? ids[0] : ids[ids.length - 1];
      if (edge !== undefined && edge !== entry.id) focusEntry(edge);
    } else if (event.key === "Enter" || (selecting && event.key === " ")) {
      event.preventDefault();
      if (selecting) toggleSelect(entry.id);
      else openDetail(entry);
    } else if (
      (event.ctrlKey || event.metaKey) &&
      event.key.toLowerCase() === "c"
    ) {
      event.preventDefault();
      void actions.copy(entry);
    }
  };

  const openDetail = (entry: HistoryEntry) => {
    lastFocusedRef.current = entry.id;
    setDetailId(entry.id);
  };

  const closeDetail = () => {
    setDetailId(null);
    const id = lastFocusedRef.current;
    requestAnimationFrame(
      () =>
        (id !== null ? document.getElementById(`history-entry-${id}`) : null) ??
        listRef.current?.focus(),
    );
  };

  useEffect(() => {
    if (detailId !== null) drawerRef.current?.focus();
  }, [detailId]);

  const detailEntry =
    feed.entries.find((entry) => entry.id === detailId) ?? null;
  const activeModelName =
    models.find((model) => model.id === currentModel)?.name ?? null;
  const streakDays = useMemo(
    () => computeStreak(feed.streakTimestamps, now),
    [feed.streakTimestamps, now],
  );
  const filtering =
    feed.query.trim() !== "" || Object.values(feed.filters).some(Boolean);
  const goToModels = () =>
    navigate({ section: "settings", settingsTab: "transcription/models" });

  const dayLabel = (row: Extract<HomeRow, { kind: "heading" }>) =>
    row.dayKind === "today"
      ? t("home.day.today")
      : row.dayKind === "yesterday"
        ? t("home.day.yesterday")
        : dayFormatter.format(row.date);

  return (
    <main className="home-root" aria-busy={feed.loading}>
      <div ref={scrollRef} className="home-page">
        <header className="home-head">
          <h1>{t(`home.greeting.${greetingPeriod(now.getHours())}`)}</h1>
          <div className="home-head-actions">
            <button
              type="button"
              className="btn-secondary"
              onClick={() => navigate({ section: "help" })}
            >
              <Keyboard width={14} height={14} aria-hidden="true" />
              {t("home.shortcuts")}
            </button>
            <button
              type="button"
              className="btn-secondary"
              onClick={() => void commands.openRecordingsFolder()}
            >
              <FolderOpen width={14} height={14} aria-hidden="true" />
              {t("settings.history.openFolder")}
            </button>
          </div>
        </header>

        <div className="home-grid">
          <div className="home-main">
            {showBanner ? (
              <HomeBanner
                onChooseModel={goToModels}
                onDismiss={() => void dismiss(BANNER_ID)}
              />
            ) : null}

            <div className="home-toolbar">
              <label className="search-field">
                <span className="sr-only">
                  {t("settings.history.searchLabel")}
                </span>
                <Search aria-hidden="true" width={16} height={16} />
                <input
                  value={feed.query}
                  onChange={(event) => feed.setQuery(event.target.value)}
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
                    className="select-field"
                    value={feed.filters[name]}
                    onChange={(event) =>
                      feed.setFilters({
                        ...feed.filters,
                        [name]: event.target.value,
                      })
                    }
                  >
                    <option value="">
                      {t(`settings.history.filters.${name}`)}
                    </option>
                    {name === "app" &&
                      feed.apps.map((app) => (
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
                        "routed",
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
              <button
                type="button"
                className="btn-secondary home-select-toggle"
                aria-pressed={selecting}
                onClick={() =>
                  selecting ? exitSelection() : setSelecting(true)
                }
              >
                <ListChecks width={14} height={14} aria-hidden="true" />
                {t("settings.history.select")}
              </button>
            </div>

            {selecting ? (
              <div className="home-selectbar" aria-busy={deletingSelected}>
                <label className="home-selectall">
                  <input
                    type="checkbox"
                    checked={allState === "all"}
                    aria-label={t("settings.history.selectAll")}
                    ref={(input) => {
                      if (input) input.indeterminate = allState === "some";
                    }}
                    onChange={() =>
                      setSelected((current) =>
                        applySelectAll(entryIds, current),
                      )
                    }
                  />
                  {t("settings.history.selectAll")}
                </label>
                <span className="home-selectbar-spacer" />
                <button
                  type="button"
                  className="btn-secondary home-danger"
                  disabled={selectedIds.length === 0 || deletingSelected}
                  onClick={() => void deleteSelected()}
                >
                  <Trash2 width={14} height={14} aria-hidden="true" />
                  {t("settings.history.deleteSelected", {
                    count: selectedIds.length,
                  })}
                </button>
              </div>
            ) : null}

            <div
              ref={listRef}
              tabIndex={-1}
              className="home-list"
              role="region"
              aria-label={t("settings.history.listLabel")}
              aria-busy={feed.loading || feed.loadingMore}
            >
              <p className="sr-only" role="status" aria-live="polite">
                {feed.loading
                  ? t("settings.history.loading")
                  : t("settings.history.resultCount", {
                      count: feed.entries.length,
                    })}
              </p>
              {feed.loading ? (
                <p className="home-note">{t("settings.history.loading")}</p>
              ) : null}
              {!feed.loading && rows.length === 0 && !feed.loadError ? (
                filtering ? (
                  <p className="home-note">{t("settings.history.empty")}</p>
                ) : (
                  <div className="home-empty" data-testid="home-empty">
                    <h2>{t("home.empty.title")}</h2>
                    <p>{t("home.empty.body")}</p>
                  </div>
                )
              ) : null}
              {feed.loadError ? (
                <p className="home-note" role="alert">
                  {t("settings.history.loadError")}
                </p>
              ) : null}
              <div
                className="home-virtual"
                style={{ height: virtualizer.getTotalSize() }}
              >
                {virtualItems.map((virtualRow) => {
                  const row = rows[virtualRow.index];
                  return (
                    <div
                      key={row.key}
                      ref={virtualizer.measureElement}
                      data-index={virtualRow.index}
                      className="home-virtual-row"
                      style={{
                        transform: `translateY(${virtualRow.start - scrollMargin}px)`,
                      }}
                    >
                      {row.kind === "heading" ? (
                        <h2 className="day-head caps">{dayLabel(row)}</h2>
                      ) : (
                        <HistoryRow
                          entry={row.entry}
                          position={row.position}
                          tabbable={
                            (activeId ?? feed.entries[0]?.id) === row.entry.id
                          }
                          selecting={selecting}
                          selected={selected.has(row.entry.id)}
                          onToggleSelect={(entry) => toggleSelect(entry.id)}
                          onActivate={setActiveId}
                          retrying={actions.retrying === row.entry.id}
                          onOpen={openDetail}
                          onCopy={(entry) => void actions.copy(entry)}
                          onToggleFlag={(entry) =>
                            void actions.toggleFlag(entry)
                          }
                          onRetry={(entry) => void actions.retry(entry)}
                          onKeyDown={handleRowKeyDown}
                        />
                      )}
                    </div>
                  );
                })}
              </div>
              {feed.loadingMore ? (
                <p className="home-note">{t("settings.history.loadingMore")}</p>
              ) : null}
            </div>
          </div>

          <StatsCard
            words={feed.stats?.words_total ?? 0}
            wordsPerMinute={feed.stats?.words_per_minute ?? 0}
            streakDays={streakDays}
            secondsSaved={feed.stats?.seconds_saved ?? 0}
            activeModelName={activeModelName}
            tipVisible={showTip}
            onManageModels={goToModels}
            onDismissTip={() => void dismiss(TIP_ID)}
          />
        </div>
      </div>

      {detailEntry ? (
        <aside
          ref={drawerRef}
          className="home-drawer"
          aria-label={t("settings.history.detail.label")}
          tabIndex={-1}
          onKeyDown={(event) => {
            if (event.key === "Escape") {
              event.stopPropagation();
              closeDetail();
            }
          }}
        >
          <button
            type="button"
            className="icon-button drawer-close"
            aria-label={t("home.detail.close")}
            onClick={closeDetail}
          >
            <X width={16} height={16} aria-hidden="true" />
          </button>
          <HistoryDetail
            entry={detailEntry}
            retrying={actions.retrying === detailEntry.id}
            retryModels={actions.retryModels}
            retryModelId={actions.retryModelId}
            onRetryModelChange={actions.setRetryModelId}
            onCopy={() => void actions.copy(detailEntry)}
            onReinsert={() => void actions.reinsert(detailEntry)}
            onRetry={() => void actions.retry(detailEntry)}
            onDelete={() => void actions.remove(detailEntry)}
            onToggleFlag={() => void actions.toggleFlag(detailEntry)}
            onAddDictionary={(term) => void actions.addToDictionary(term)}
          />
        </aside>
      ) : null}
    </main>
  );
};
