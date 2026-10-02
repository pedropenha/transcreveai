import React, {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { emit } from "@tauri-apps/api/event";
import { Search, Settings, ShieldCheck } from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { commands, type MeetingListItem } from "@/bindings";
import { useNow } from "@/hooks/useNow";
import { useSettings } from "@/hooks/useSettings";
import { useHubNavigation } from "../shell/HubNavigation";
import { SUMMARY_SETTINGS_TAB } from "../shell/navModel";
import { SEARCH_DEBOUNCE_MS } from "../settings/meetings/meetingsView";
import { LiveBlock } from "./LiveBlock";
import { MeetingDetailPanel } from "./MeetingDetailPanel";
import { MeetingRow } from "./MeetingRow";
import { StartMeetingMenu } from "./StartMeetingMenu";
import {
  countProcessing,
  detectionStatus,
  groupMeetingsByDay,
  itemsForTab,
  retryActionFor,
  splitMeetings,
  type NotetakerTab,
} from "./notetakerView";
import { useMeetingDetail } from "./useMeetingDetail";
import { useMeetingFeed } from "./useMeetingFeed";
import "./notetaker.css";

const detailVersion = (item: MeetingListItem | null): string =>
  item
    ? `${item.list_status}|${item.title}|${item.summary_status}|${item.ended_at ?? ""}`
    : "";

/**
 * Notetaker (F009/F010, proposal §Notetaker): "Agora" on top, past notes
 * grouped by day with the logo of the app they happened in, details of the
 * selected meeting on the right. No calendar (P2).
 */
export const NotetakerPage: React.FC = () => {
  const { t, i18n } = useTranslation();
  const navigate = useHubNavigation();
  const now = useNow();
  const { settings } = useSettings();

  const [query, setQuery] = useState("");
  const [debouncedQuery, setDebouncedQuery] = useState("");
  const [searchOpen, setSearchOpen] = useState(false);
  const [tab, setTab] = useState<NotetakerTab>("past");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [retryingId, setRetryingId] = useState<string | null>(null);
  const searchRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    const handle = setTimeout(
      () => setDebouncedQuery(query),
      SEARCH_DEBOUNCE_MS,
    );
    return () => clearTimeout(handle);
  }, [query]);
  useEffect(() => {
    if (searchOpen) searchRef.current?.focus();
  }, [searchOpen]);

  const feed = useMeetingFeed(debouncedQuery);
  const { live, past } = useMemo(() => splitMeetings(feed.items), [feed.items]);
  const processingCount = countProcessing(past);
  // The "Transcrevendo" tab empties itself once the last meeting finishes.
  const activeTab: NotetakerTab = processingCount === 0 ? "past" : tab;
  const visible = useMemo(
    () => itemsForTab(past, activeTab),
    [past, activeTab],
  );
  const groups = useMemo(
    () => groupMeetingsByDay(visible, now),
    [visible, now],
  );

  const selected =
    feed.items.find((m) => m.id === selectedId && m !== live) ?? null;
  const detail = useMeetingDetail(
    selected?.id ?? null,
    detailVersion(selected),
  );

  const dayFormatter = useMemo(
    () =>
      new Intl.DateTimeFormat(i18n.language, {
        weekday: "long",
        day: "numeric",
        month: "long",
      }),
    [i18n.language],
  );
  const dayLabel = (kind: "today" | "yesterday" | "date", date: Date) =>
    kind === "today"
      ? t("home.day.today")
      : kind === "yesterday"
        ? t("home.day.yesterday")
        : dayFormatter.format(date);

  const openMeetingWindow = useCallback(async (id: string | null) => {
    try {
      const result = await commands.meetingWindowOpen(id);
      if (result.status !== "ok") {
        console.warn("meeting_window_open failed:", result.error);
      }
    } catch (e) {
      console.warn("meeting_window_open invoke failed:", e);
    }
  }, []);

  const startMeeting = useCallback(
    async (micOnly: boolean) => {
      try {
        const result = await commands.meetingStart(micOnly);
        if (result.status === "ok") {
          await openMeetingWindow(result.data.id);
        } else if (result.error.code === "consent_required") {
          // Same lane as the Flow Bar/tray: MeetingConsentGate listens.
          await emit("toast://show", {
            kind: "meeting_consent",
            message: result.error.message,
            action: "open_consent",
          });
        } else {
          toast.error(result.error.message);
        }
      } catch (e) {
        console.warn("meeting_start invoke failed:", e);
        toast.error(t("notetaker.startError"));
      }
    },
    [openMeetingWindow, t],
  );

  const deleteMeeting = useCallback(
    async (id: string) => {
      feed.removeLocal(id);
      setSelectedId((current) => (current === id ? null : current));
      try {
        const result = await commands.meetingDelete(id);
        if (result.status !== "ok") {
          toast.error(t("settings.meetings.deleteError"));
          await feed.refresh();
        }
      } catch (e) {
        console.warn("meeting_delete invoke failed:", e);
        toast.error(t("settings.meetings.deleteError"));
        await feed.refresh();
      }
    },
    [feed.refresh, feed.removeLocal, t],
  );

  const retry = useCallback(
    async (item: MeetingListItem) => {
      const action = retryActionFor(item);
      if (action === null) return;
      setRetryingId(item.id);
      try {
        const result =
          action === "retry_processing"
            ? await commands.meetingRetryProcessing(item.id)
            : await commands.meetingRegenerateSummary(item.id);
        if (result.status !== "ok") {
          toast.error(result.error.message || t("notetaker.retryError"));
        }
      } catch (e) {
        console.warn("meeting retry invoke failed:", e);
        toast.error(t("notetaker.retryError"));
      } finally {
        setRetryingId(null);
        await feed.refresh();
      }
    },
    [feed.refresh, feed.removeLocal, t],
  );

  const openSummarySettings = useCallback(
    () => navigate({ section: "settings", settingsTab: SUMMARY_SETTINGS_TAB }),
    [navigate],
  );

  const detection = detectionStatus(
    {
      enabled: settings?.meeting_detection_enabled ?? true,
      pausedUntilMs: settings?.meeting_detection_paused_until_ms ?? null,
      offline: settings?.offline_mode ?? false,
    },
    now.getTime(),
  );
  const searching = debouncedQuery.trim() !== "";

  return (
    <div className="nt-root">
      <div className="nt-page">
        <header className="nt-head">
          <h1>{t("sidebar.meetings")}</h1>
          <button
            type="button"
            className="icon-button"
            aria-label={t("notetaker.settings")}
            title={t("notetaker.settings")}
            onClick={openSummarySettings}
          >
            <Settings width={17} height={17} aria-hidden="true" />
          </button>
          <StartMeetingMenu onStart={(micOnly) => void startMeeting(micOnly)} />
        </header>

        {live ? (
          <LiveBlock
            item={live}
            elapsedMs={
              feed.liveState?.meeting_id === live.id
                ? feed.liveState.elapsed_ms
                : null
            }
            onOpen={(id) => void openMeetingWindow(id)}
          />
        ) : null}

        <p className="nt-detect" data-status={detection}>
          <ShieldCheck width={14} height={14} aria-hidden="true" />
          <span>{t(`notetaker.detection.${detection}`)}</span>
        </p>

        <div className="nt-tabs">
          <div
            role="group"
            aria-label={t("notetaker.tabs")}
            className="nt-tabset"
          >
            <button
              type="button"
              aria-pressed={activeTab === "past"}
              onClick={() => setTab("past")}
            >
              {t("notetaker.tab.past")}
            </button>
            {processingCount > 0 ? (
              <button
                type="button"
                aria-pressed={activeTab === "processing"}
                onClick={() => setTab("processing")}
              >
                {t("notetaker.tab.processing")}
                <span className="nt-count">{processingCount}</span>
              </button>
            ) : null}
          </div>
          <span className="nt-grow" />
          <button
            type="button"
            className="icon-button"
            aria-label={t("notetaker.search")}
            aria-expanded={searchOpen}
            title={t("notetaker.search")}
            onClick={() => setSearchOpen((open) => !open)}
          >
            <Search width={16} height={16} aria-hidden="true" />
          </button>
        </div>

        {searchOpen || query !== "" ? (
          <div className="search-field nt-search">
            <Search width={15} height={15} aria-hidden="true" />
            <input
              ref={searchRef}
              type="search"
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              placeholder={t("settings.meetings.searchPlaceholder")}
              aria-label={t("settings.meetings.searchPlaceholder")}
            />
          </div>
        ) : null}

        <div aria-busy={feed.loading}>
          {feed.loading && feed.items.length === 0 ? (
            <p className="nt-note-center">{t("settings.meetings.loading")}</p>
          ) : groups.length === 0 ? (
            <EmptyState
              searching={searching}
              hasLive={live !== null}
              tab={activeTab}
              onStart={() => void startMeeting(false)}
            />
          ) : (
            groups.map((group) => (
              <section className="nt-group" key={group.key}>
                <h2 className="caps">{dayLabel(group.kind, group.date)}</h2>
                <ul>
                  {group.entries.map((item) => (
                    <MeetingRow
                      key={item.id}
                      item={item}
                      locale={i18n.language}
                      selected={item.id === selected?.id}
                      progressPct={feed.progress.get(item.id) ?? null}
                      retrying={retryingId === item.id}
                      onSelect={setSelectedId}
                      onRetry={(row) => void retry(row)}
                      onOpenSummarySettings={openSummarySettings}
                    />
                  ))}
                </ul>
              </section>
            ))
          )}
        </div>
      </div>

      {selected ? (
        <MeetingDetailPanel
          key={selected.id}
          item={selected}
          detail={detail}
          locale={i18n.language}
          retrying={retryingId === selected.id}
          onClose={() => setSelectedId(null)}
          onOpenTranscript={(id) => void openMeetingWindow(id)}
          onRetry={(row) => void retry(row)}
          onRenamed={() => void feed.refresh()}
          onDelete={(id) => void deleteMeeting(id)}
          onOpenSummarySettings={openSummarySettings}
        />
      ) : null}
    </div>
  );
};

const EmptyState: React.FC<{
  searching: boolean;
  hasLive: boolean;
  tab: NotetakerTab;
  onStart: () => void;
}> = ({ searching, hasLive, tab, onStart }) => {
  const { t } = useTranslation();
  if (searching) {
    return (
      <p className="nt-note-center">{t("settings.meetings.emptySearch")}</p>
    );
  }
  if (tab === "processing") {
    return <p className="nt-note-center">{t("notetaker.empty.processing")}</p>;
  }
  if (hasLive) {
    return <p className="nt-note-center">{t("notetaker.empty.afterLive")}</p>;
  }
  return (
    <div className="nt-empty" data-testid="notetaker-empty">
      <h2>{t("notetaker.empty.title")}</h2>
      <p>{t("notetaker.empty.body")}</p>
      <button type="button" className="nt-btn nt-btn-rec" onClick={onStart}>
        <span className="nt-btn-dot" aria-hidden="true" />
        {t("notetaker.start")}
      </button>
    </div>
  );
};
