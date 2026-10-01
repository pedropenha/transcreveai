import React, { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { Check, ChevronDown, Copy, Plus, Search, Trash2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { commands, type Meeting } from "@/bindings";
import { formatDateTime } from "@/utils/dateFormat";
import { Button } from "../../ui/Button";
import { Input } from "../../ui/Input";
import Badge from "../../ui/Badge";
import { copyToClipboard } from "../history/clipboard";
import {
  SEARCH_DEBOUNCE_MS,
  formatMeetingDuration,
  meetingAppLabel,
  meetingDurationSeconds,
  normalizeMeetingStatus,
  statusBadgeVariant,
} from "./meetingsView";

/**
 * FR-009-25 (T-068): meeting list in the Hub with full-text search over
 * title, notes, summary and transcript (`meeting_search` unions the three
 * FTS tables backend-side). `meeting_window_open` is delivered by the
 * parallel T-066 lane — we invoke it by name and degrade to a warn until it
 * lands, so both orders of the merge converge.
 */
export const MeetingsSettings: React.FC = () => {
  const { t, i18n } = useTranslation();
  const [meetings, setMeetings] = useState<Meeting[]>([]);
  const [loading, setLoading] = useState(true);
  const [query, setQuery] = useState("");
  const [debouncedQuery, setDebouncedQuery] = useState("");

  // Debounce the search box so FTS only runs once the user pauses typing.
  useEffect(() => {
    const handle = setTimeout(
      () => setDebouncedQuery(query),
      SEARCH_DEBOUNCE_MS,
    );
    return () => clearTimeout(handle);
  }, [query]);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    void commands
      .meetingSearch(debouncedQuery)
      .then((result) => {
        if (cancelled) return;
        if (result.status === "ok") {
          setMeetings(result.data);
        } else {
          console.warn("meeting_search failed:", result.error);
        }
      })
      .catch((e) => console.warn("meeting_search invoke failed:", e))
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [debouncedQuery]);

  const openMeetingWindow = useCallback(async (meetingId: string) => {
    try {
      await invoke("meeting_window_open", { meetingId });
    } catch (e) {
      // T-066 lane: the command may not exist yet at merge time.
      console.warn("meeting_window_open is unavailable:", e);
    }
  }, []);

  const startMeeting = useCallback(
    async (micOnly: boolean) => {
      try {
        const result = await commands.meetingStart(micOnly);
        if (result.status === "ok") {
          await openMeetingWindow(result.data.id);
        } else if (result.error.code === "consent_required") {
          // Surface refusals the same way the Flow Bar/tray lane does —
          // MeetingConsentGate listens for this toast event.
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
      }
    },
    [openMeetingWindow],
  );

  const deleteMeeting = useCallback(
    async (meetingId: string) => {
      // Optimistic remove; reload the list on failure.
      setMeetings((prev) => prev.filter((m) => m.id !== meetingId));
      try {
        const result = await commands.meetingDelete(meetingId);
        if (result.status !== "ok") {
          toast.error(t("settings.meetings.deleteError"));
          const reload = await commands.meetingSearch(debouncedQuery);
          if (reload.status === "ok") setMeetings(reload.data);
        }
      } catch (e) {
        console.warn("meeting_delete invoke failed:", e);
        toast.error(t("settings.meetings.deleteError"));
      }
    },
    [debouncedQuery, t],
  );

  let content: React.ReactNode;
  if (loading && meetings.length === 0) {
    content = (
      <div className="px-4 py-3 text-center text-text/60">
        {t("settings.meetings.loading")}
      </div>
    );
  } else if (meetings.length === 0) {
    content = (
      <div className="px-4 py-3 text-center text-text/60">
        {debouncedQuery.trim()
          ? t("settings.meetings.emptySearch")
          : t("settings.meetings.empty")}
      </div>
    );
  } else {
    content = (
      <div className="divide-y divide-mid-gray/20">
        {meetings.map((meeting) => (
          <MeetingRow
            key={meeting.id}
            meeting={meeting}
            locale={i18n.language}
            onOpen={() => openMeetingWindow(meeting.id)}
            onDelete={() => deleteMeeting(meeting.id)}
          />
        ))}
      </div>
    );
  }

  return (
    <div className="max-w-3xl w-full mx-auto space-y-6">
      <div className="space-y-2">
        <div className="px-4 flex items-center justify-between gap-3">
          <h2 className="text-xs font-medium text-mid-gray uppercase tracking-wide">
            {t("settings.meetings.title")}
          </h2>
          <NewMeetingButton onStart={startMeeting} />
        </div>
        <div className="px-4">
          <div className="relative">
            <Search className="absolute start-3 top-1/2 -translate-y-1/2 w-4 h-4 text-mid-gray pointer-events-none" />
            <Input
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder={t("settings.meetings.searchPlaceholder")}
              aria-label={t("settings.meetings.searchPlaceholder")}
              className="w-full ps-9"
            />
          </div>
        </div>
        <div className="bg-background border border-mid-gray/20 rounded-lg overflow-visible">
          {content}
        </div>
      </div>
    </div>
  );
};

/** FR-009-01: Hub "Nova reunião" — Chamada no computador (mic + system) or
 * Presencial (mic only). */
const NewMeetingButton: React.FC<{
  onStart: (micOnly: boolean) => void;
}> = ({ onStart }) => {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const handleClickOutside = (event: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(event.target as Node)) {
        setOpen(false);
      }
    };
    document.addEventListener("mousedown", handleClickOutside);
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, [open]);

  const options: { labelKey: string; micOnly: boolean }[] = [
    { labelKey: "settings.meetings.newMeetingCall", micOnly: false },
    { labelKey: "settings.meetings.newMeetingInPerson", micOnly: true },
  ];

  return (
    <div className="relative" ref={menuRef}>
      <Button
        variant="secondary"
        size="sm"
        className="flex items-center gap-2"
        onClick={() => setOpen((v) => !v)}
        aria-haspopup="menu"
        aria-expanded={open}
      >
        <Plus className="w-4 h-4" />
        <span>{t("settings.meetings.newMeeting")}</span>
        <ChevronDown className="w-3 h-3" />
      </Button>
      {open && (
        <div
          role="menu"
          className="absolute end-0 top-full mt-1 bg-background border border-mid-gray/80 rounded-md shadow-lg z-50 min-w-[220px] overflow-hidden"
        >
          {options.map((option) => (
            <button
              key={option.labelKey}
              type="button"
              role="menuitem"
              className="w-full px-3 py-2 text-sm text-start hover:bg-logo-primary/10 transition-colors duration-150 cursor-pointer"
              onClick={() => {
                setOpen(false);
                onStart(option.micOnly);
              }}
            >
              {t(option.labelKey)}
            </button>
          ))}
        </div>
      )}
    </div>
  );
};

const MeetingRow: React.FC<{
  meeting: Meeting;
  locale: string;
  onOpen: () => void;
  onDelete: () => void;
}> = ({ meeting, locale, onOpen, onDelete }) => {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  const [copying, setCopying] = useState(false);

  const copyMarkdown = async () => {
    setCopying(true);
    try {
      const result = await commands.meetingExportMarkdown(meeting.id);
      if (result.status === "ok" && (await copyToClipboard(result.data))) {
        setCopied(true);
        setTimeout(() => setCopied(false), 2000);
      } else {
        toast.error(t("settings.meetings.copyError"));
      }
    } catch (e) {
      console.warn("meeting_export_markdown failed:", e);
      toast.error(t("settings.meetings.copyError"));
    } finally {
      setCopying(false);
    }
  };

  const status = normalizeMeetingStatus(meeting.status);
  const meta = [
    formatDateTime(String(meeting.started_at), locale),
    formatMeetingDuration(meetingDurationSeconds(meeting)),
    meetingAppLabel(meeting),
  ].join(" · ");

  return (
    <div
      className="px-4 py-3 flex items-center gap-3 cursor-pointer hover:bg-mid-gray/10 transition-colors"
      onClick={onOpen}
      role="button"
      tabIndex={0}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onOpen();
        }
      }}
      aria-label={t("settings.meetings.open", { title: meeting.title })}
    >
      <div className="flex-1 min-w-0">
        <div className="flex items-center gap-2">
          <p className="text-sm font-medium truncate">{meeting.title}</p>
          <Badge variant={statusBadgeVariant(status)} className="shrink-0">
            {t(`settings.meetings.status.${status}`)}
          </Badge>
        </div>
        <p className="text-xs text-mid-gray mt-0.5 truncate">{meta}</p>
      </div>
      <div
        className="flex items-center shrink-0"
        onClick={(e) => e.stopPropagation()}
      >
        <button
          type="button"
          onClick={copyMarkdown}
          disabled={copying}
          title={t("settings.meetings.copyMarkdown")}
          className="p-1.5 rounded-md flex items-center justify-center transition-colors cursor-pointer disabled:cursor-not-allowed disabled:text-text/20 text-text/50 hover:text-logo-primary"
        >
          {copied ? (
            <Check width={16} height={16} />
          ) : (
            <Copy width={16} height={16} />
          )}
        </button>
        <button
          type="button"
          onClick={onDelete}
          title={t("settings.meetings.delete")}
          className="p-1.5 rounded-md flex items-center justify-center transition-colors cursor-pointer text-text/50 hover:text-red-400"
        >
          <Trash2 width={16} height={16} />
        </button>
      </div>
    </div>
  );
};
