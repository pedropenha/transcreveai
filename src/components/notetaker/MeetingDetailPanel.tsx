import React, { useEffect, useRef, useState } from "react";
import {
  ArrowRight,
  Check,
  Copy,
  KeyRound,
  Mic,
  Pencil,
  RefreshCw,
  Sparkles,
  Trash2,
  Volume2,
  X,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { commands, type MeetingListItem } from "@/bindings";
import { copyToClipboard } from "../settings/history/clipboard";
import { AppLogo } from "./AppLogo";
import { meetingMetaLine } from "./MeetingRow";
import { SummaryMarkdown } from "./SummaryMarkdown";
import { MeetingConnectorActions } from "./MeetingConnectorActions";
import {
  retryActionFor,
  summarySections,
  transcriptExcerpt,
} from "./notetakerView";
import type { DetailState } from "./useMeetingDetail";

const EXCERPT_LINES = 3;
const TITLE_MAX_CHARS = 160;
const COPIED_FLASH_MS = 2000;

interface MeetingDetailPanelProps {
  item: MeetingListItem;
  detail: DetailState;
  locale: string;
  retrying: boolean;
  onClose: () => void;
  onOpenTranscript: (id: string) => void;
  onRetry: (item: MeetingListItem) => void;
  onRenamed: () => void;
  onDelete: (id: string) => void;
  onOpenSummarySettings: () => void;
}

/** Right-hand details of the selected meeting (proposal §Notetaker). */
export const MeetingDetailPanel: React.FC<MeetingDetailPanelProps> = ({
  item,
  detail,
  locale,
  retrying,
  onClose,
  onOpenTranscript,
  onRetry,
  onRenamed,
  onDelete,
  onOpenSummarySettings,
}) => {
  const { t } = useTranslation();
  const [copied, setCopied] = useState<"summary" | "markdown" | null>(null);
  const [renaming, setRenaming] = useState(false);
  const [draft, setDraft] = useState(item.title);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const renameSettled = useRef(false);
  const copiedTimer = useRef<number | undefined>(undefined);

  useEffect(() => () => window.clearTimeout(copiedTimer.current), []);

  const flashCopied = (what: "summary" | "markdown") => {
    setCopied(what);
    window.clearTimeout(copiedTimer.current);
    copiedTimer.current = window.setTimeout(
      () => setCopied(null),
      COPIED_FLASH_MS,
    );
  };

  const copySummary = async () => {
    if (item.summary_md && (await copyToClipboard(item.summary_md))) {
      flashCopied("summary");
    } else {
      toast.error(t("notetaker.copyError"));
    }
  };

  const copyMarkdown = async () => {
    try {
      const result = await commands.meetingExportMarkdown(item.id);
      if (result.status === "ok" && (await copyToClipboard(result.data))) {
        flashCopied("markdown");
        return;
      }
    } catch (e) {
      console.warn("meeting_export_markdown failed:", e);
    }
    toast.error(t("settings.meetings.copyError"));
  };

  const submitRename = async () => {
    // Enter, Escape and the blur they cause all land here: settle once.
    if (renameSettled.current) return;
    renameSettled.current = true;
    const title = draft.trim().slice(0, TITLE_MAX_CHARS);
    setRenaming(false);
    if (title === "" || title === item.title) return;
    try {
      const result = await commands.meetingRename(item.id, title);
      if (result.status === "ok") onRenamed();
      else toast.error(t("notetaker.renameError"));
    } catch (e) {
      console.warn("meeting_rename invoke failed:", e);
      toast.error(t("notetaker.renameError"));
    }
  };

  const ready =
    detail.phase === "ready" && detail.detail.meeting.id === item.id
      ? detail.detail
      : null;
  const sections = summarySections(item.summary_md);
  const excerpt = ready ? transcriptExcerpt(ready.segments, EXCERPT_LINES) : [];
  const failed = retryActionFor(item) !== null;
  const hasSummary = sections.length > 0;

  return (
    <aside
      className="nt-detail"
      aria-label={t("notetaker.detail.label")}
      onKeyDown={(event) => {
        if (event.key === "Escape" && !renaming) onClose();
      }}
    >
      <div className="nt-detail-top">
        <AppLogo meetingId={item.id} app={item.source_app} size="lg" />
        <button
          type="button"
          className="icon-button"
          aria-label={t("notetaker.detail.close")}
          title={t("notetaker.detail.close")}
          onClick={onClose}
        >
          <X width={16} height={16} aria-hidden="true" />
        </button>
      </div>

      {renaming ? (
        <form
          className="nt-rename"
          onSubmit={(event) => {
            event.preventDefault();
            void submitRename();
          }}
        >
          <input
            autoFocus
            value={draft}
            maxLength={TITLE_MAX_CHARS}
            aria-label={t("notetaker.detail.rename")}
            onChange={(event) => setDraft(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Escape") {
                event.stopPropagation();
                renameSettled.current = true;
                setRenaming(false);
                setDraft(item.title);
              }
            }}
            onBlur={() => void submitRename()}
          />
        </form>
      ) : (
        <div className="nt-detail-titlerow">
          <h2 className="nt-detail-title">{item.title}</h2>
          <button
            type="button"
            className="icon-button nt-rename-btn"
            aria-label={t("notetaker.detail.rename")}
            title={t("notetaker.detail.rename")}
            onClick={() => {
              renameSettled.current = false;
              setRenaming(true);
            }}
          >
            <Pencil width={14} height={14} aria-hidden="true" />
          </button>
        </div>
      )}
      <p className="nt-detail-sub">
        {[item.source_app?.name, meetingMetaLine(item, locale)]
          .filter(Boolean)
          .join(" · ")}
      </p>

      <div className="nt-acts">
        {hasSummary ? (
          <button
            type="button"
            className="btn-secondary"
            onClick={() => void copySummary()}
          >
            {copied === "summary" ? (
              <Check width={14} height={14} aria-hidden="true" />
            ) : (
              <Copy width={14} height={14} aria-hidden="true" />
            )}
            {t("notetaker.detail.copySummary")}
          </button>
        ) : null}
        <button
          type="button"
          className="btn-secondary"
          onClick={() => void copyMarkdown()}
        >
          {copied === "markdown" ? (
            <Check width={14} height={14} aria-hidden="true" />
          ) : (
            <Copy width={14} height={14} aria-hidden="true" />
          )}
          {t("settings.meetings.copyMarkdown")}
        </button>
        {confirmDelete ? (
          <button
            type="button"
            className="btn-secondary nt-danger"
            onClick={() => onDelete(item.id)}
            onBlur={() => setConfirmDelete(false)}
            autoFocus
          >
            <Trash2 width={14} height={14} aria-hidden="true" />
            {t("notetaker.detail.confirmDelete")}
          </button>
        ) : (
          <button
            type="button"
            className="icon-button"
            aria-label={t("settings.meetings.delete")}
            title={t("settings.meetings.delete")}
            onClick={() => setConfirmDelete(true)}
          >
            <Trash2 width={16} height={16} aria-hidden="true" />
          </button>
        )}
      </div>

      {item.list_status === "processing" ? (
        <p className="nt-note" role="status">
          {t("notetaker.detail.processing")}
        </p>
      ) : null}

      {failed ? (
        <div className="nt-note nt-note-error" role="alert">
          <p>{t("notetaker.detail.failed")}</p>
          <button
            type="button"
            className="link-button"
            disabled={retrying}
            onClick={() => onRetry(item)}
          >
            <RefreshCw width={13} height={13} aria-hidden="true" />
            {t("notetaker.retry")}
          </button>
        </div>
      ) : null}

      {item.list_status === "no_summary" ? (
        <div className="nt-note">
          <p>{t("notetaker.detail.noSummary")}</p>
          <button
            type="button"
            className="link-button"
            onClick={onOpenSummarySettings}
          >
            <KeyRound width={13} height={13} aria-hidden="true" />
            {t("notetaker.detail.configureSummary")}
          </button>
        </div>
      ) : null}

      {sections.map((section, index) => (
        <section className="nt-sec" key={`${index}-${section.heading ?? ""}`}>
          <h3 className="caps">
            {section.heading ?? t("notetaker.detail.summary")}
          </h3>
          <SummaryMarkdown markdown={section.body} />
        </section>
      ))}

      <MeetingConnectorActions meetingId={item.id} hasSummary={hasSummary} />

      <section className="nt-sec">
        <h3 className="caps">{t("notetaker.detail.transcript")}</h3>
        {detail.phase === "error" || detail.phase === "missing" ? (
          <p className="nt-muted" role="alert">
            {t("notetaker.detail.loadError")}
          </p>
        ) : excerpt.length === 0 ? (
          <p className="nt-muted">
            {ready ? t("notetaker.detail.noTranscript") : "…"}
          </p>
        ) : (
          excerpt.map((line) => (
            <p className="nt-tr-line" key={line.id}>
              <time>{line.time}</time>
              <span>{line.text}</span>
            </p>
          ))
        )}
        <button
          type="button"
          className="link-button nt-open-full"
          onClick={() => onOpenTranscript(item.id)}
        >
          {t("notetaker.detail.openFull")}
          <ArrowRight width={13} height={13} aria-hidden="true" />
        </button>
      </section>

      <section className="nt-sec">
        <h3 className="caps">{t("notetaker.detail.processingInfo")}</h3>
        <div className="nt-src">
          <span className="nt-tag">
            <Mic width={12} height={12} aria-hidden="true" />
            {t("notetaker.detail.microphone")}
          </span>
          {item.capture_system_audio ? (
            <span className="nt-tag">
              <Volume2 width={12} height={12} aria-hidden="true" />
              {t("notetaker.detail.system")}
            </span>
          ) : null}
          {item.stt_provider_id ? (
            <span className="nt-tag">{item.stt_provider_id}</span>
          ) : null}
          {item.llm_provider_id ? (
            <span className="nt-tag">
              <Sparkles width={12} height={12} aria-hidden="true" />
              {t("notetaker.detail.summaryProvider", {
                provider: item.llm_provider_id,
              })}
            </span>
          ) : null}
        </div>
      </section>
    </aside>
  );
};
