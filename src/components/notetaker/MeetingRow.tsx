import React from "react";
import { Check, KeyRound, RefreshCw } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { MeetingListItem } from "@/bindings";
import {
  formatMeetingDuration,
  meetingDurationSeconds,
} from "../settings/meetings/meetingsView";
import { AppLogo } from "./AppLogo";
import { retryActionFor, rowChip } from "./notetakerView";

interface MeetingRowProps {
  item: MeetingListItem;
  progressPct: number | null;
  selected: boolean;
  retrying: boolean;
  locale: string;
  onSelect: (id: string) => void;
  onRetry: (item: MeetingListItem) => void;
  onOpenSummarySettings: () => void;
}

const clockFormat: Intl.DateTimeFormatOptions = {
  hour: "2-digit",
  minute: "2-digit",
};

/** `10:00 – 10:32 · 32 min` (end and duration are absent while recording). */
export function meetingMetaLine(item: MeetingListItem, locale: string): string {
  const start = new Date(item.started_at * 1000).toLocaleTimeString(
    locale,
    clockFormat,
  );
  const end =
    item.ended_at === null
      ? null
      : new Date(item.ended_at * 1000).toLocaleTimeString(locale, clockFormat);
  const duration = meetingDurationSeconds(item);
  const range = end ? `${start} – ${end}` : start;
  return duration === null
    ? range
    : `${range} · ${formatMeetingDuration(duration)}`;
}

/** One past meeting: logo, title, time range and the single state chip. */
export const MeetingRow: React.FC<MeetingRowProps> = ({
  item,
  progressPct,
  selected,
  retrying,
  locale,
  onSelect,
  onRetry,
  onOpenSummarySettings,
}) => {
  const { t } = useTranslation();
  const chip = rowChip(item.list_status, progressPct);
  const label =
    chip.pct === null
      ? t(chip.labelKey)
      : t("notetaker.status.processingPct", { pct: chip.pct });
  const canRetry = retryActionFor(item) !== null;

  return (
    <li
      className="mt-row"
      data-selected={selected}
      data-status={item.list_status}
    >
      <button
        type="button"
        className="mt-main"
        aria-pressed={selected}
        onClick={() => onSelect(item.id)}
      >
        <AppLogo meetingId={item.id} app={item.source_app} />
        <span className="mt-text">
          <span className="mt-title">{item.title}</span>
          <span className="mt-meta">
            {meetingMetaLine(item, locale)}
            {item.source_app ? ` · ${item.source_app.name}` : ""}
          </span>
        </span>
      </button>
      <span className="mt-side">
        {canRetry ? (
          <button
            type="button"
            className="link-button"
            disabled={retrying}
            onClick={() => onRetry(item)}
          >
            <RefreshCw
              width={13}
              height={13}
              aria-hidden="true"
              className={retrying ? "animate-spin" : ""}
            />
            {t("notetaker.retry")}
          </button>
        ) : null}
        {item.list_status === "no_summary" ? (
          <button
            type="button"
            className="nt-chip"
            data-tone={chip.tone}
            title={t("notetaker.noSummaryHint")}
            onClick={onOpenSummarySettings}
          >
            <KeyRound width={12} height={12} aria-hidden="true" />
            {label}
          </button>
        ) : (
          <span className="nt-chip" data-tone={chip.tone}>
            {item.list_status === "ready" ? (
              <Check width={12} height={12} aria-hidden="true" />
            ) : null}
            {item.list_status === "processing" ? (
              <span className="nt-chip-dot" aria-hidden="true" />
            ) : null}
            {label}
          </span>
        )}
      </span>
    </li>
  );
};
