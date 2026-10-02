import React from "react";
import { Copy, Flag, MoreHorizontal, RefreshCw } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { HistoryEntry } from "@/bindings";

interface HistoryRowProps {
  entry: HistoryEntry;
  position: "first" | "middle" | "last" | "only";
  retrying: boolean;
  tabbable: boolean;
  onActivate: (id: number) => void;
  onOpen: (entry: HistoryEntry) => void;
  onCopy: (entry: HistoryEntry) => void;
  onToggleFlag: (entry: HistoryEntry) => void;
  onRetry: (entry: HistoryEntry) => void;
  onKeyDown: (
    event: React.KeyboardEvent<HTMLElement>,
    entry: HistoryEntry,
  ) => void;
}

const appLabel = (entry: HistoryEntry): string | null =>
  entry.app_name || entry.app_exe || null;

/** One dictation: time, text, meta and hover/focus actions (FR-010-01/04). */
export const HistoryRow: React.FC<HistoryRowProps> = ({
  entry,
  position,
  retrying,
  tabbable,
  onActivate,
  onOpen,
  onCopy,
  onToggleFlag,
  onRetry,
  onKeyDown,
}) => {
  const { t, i18n } = useTranslation();
  const failed = entry.status === "failed";
  const app = appLabel(entry) ?? t("settings.history.unknownApp");
  const moment = new Date(entry.timestamp * 1000);
  const time = moment.toLocaleTimeString(i18n.language, {
    hour: "2-digit",
    minute: "2-digit",
  });

  return (
    <article
      id={`history-entry-${entry.id}`}
      className="hist-row"
      data-position={position}
      data-failed={failed}
      tabIndex={tabbable ? 0 : -1}
      onFocus={() => onActivate(entry.id)}
      onClick={(event) => {
        // Inner buttons (copy, flag, retry, more) handle their own clicks.
        if ((event.target as HTMLElement).closest("button")) return;
        onOpen(entry);
      }}
      onKeyDown={(event) => onKeyDown(event, entry)}
    >
      <time className="hist-time" dateTime={moment.toISOString()}>
        {time}
      </time>
      <div className="hist-body">
        <p className="hist-text" data-empty={!entry.final_text}>
          {entry.final_text || t("settings.history.transcriptionFailed")}
        </p>
        <div className="hist-meta">
          <span className="app-tile" aria-hidden="true">
            {app.charAt(0).toUpperCase()}
          </span>
          <span>{app}</span>
          <span aria-hidden="true">·</span>
          <span>{t("home.words", { count: entry.word_count })}</span>
          {failed ? (
            <span className="chip chip-error">
              {t("settings.history.status.failed")}
            </span>
          ) : null}
          {entry.status === "copied" ? (
            <span className="chip">{t("settings.history.status.copied")}</span>
          ) : null}
          {failed && entry.audio_available ? (
            <button
              type="button"
              className="link-button"
              disabled={retrying}
              onClick={() => onRetry(entry)}
            >
              <RefreshCw
                width={13}
                height={13}
                aria-hidden="true"
                className={retrying ? "animate-spin" : ""}
              />
              {t("home.row.retry")}
            </button>
          ) : null}
        </div>
      </div>
      <div className="hist-actions">
        <button
          type="button"
          className="icon-button"
          aria-label={t("settings.history.copyToClipboard")}
          title={t("settings.history.copyToClipboard")}
          onClick={() => onCopy(entry)}
        >
          <Copy width={16} height={16} aria-hidden="true" />
        </button>
        <button
          type="button"
          className="icon-button"
          aria-label={t("settings.history.flag")}
          aria-pressed={entry.saved}
          title={t("settings.history.flag")}
          onClick={() => onToggleFlag(entry)}
        >
          <Flag
            width={16}
            height={16}
            aria-hidden="true"
            fill={entry.saved ? "currentColor" : "none"}
          />
        </button>
        <button
          type="button"
          className="icon-button"
          aria-label={t("home.row.more")}
          title={t("home.row.more")}
          onClick={() => onOpen(entry)}
        >
          <MoreHorizontal width={16} height={16} aria-hidden="true" />
        </button>
      </div>
    </article>
  );
};
