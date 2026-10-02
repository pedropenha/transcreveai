import React, { useState } from "react";
import { Pause, Play, Square } from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { commands, type MeetingListItem } from "@/bindings";
import { AppLogo } from "./AppLogo";
import { formatClock } from "./notetakerView";

interface LiveBlockProps {
  item: MeetingListItem;
  /** `meeting://state` elapsed time for this meeting, when known. */
  elapsedMs: number | null;
  onOpen: (id: string) => void;
}

const WAVE_BARS = 7;

/** "Agora": the meeting being recorded, with pause/resume and stop. */
export const LiveBlock: React.FC<LiveBlockProps> = ({
  item,
  elapsedMs,
  onOpen,
}) => {
  const { t } = useTranslation();
  const [busy, setBusy] = useState(false);
  const paused = item.list_status === "paused";
  const seconds =
    elapsedMs !== null
      ? elapsedMs / 1000
      : Math.max(0, Date.now() / 1000 - item.started_at);

  const run = async (action: () => ReturnType<typeof commands.meetingStop>) => {
    setBusy(true);
    try {
      const result = await action();
      if (result.status !== "ok") {
        console.warn("meeting control failed:", result.error);
        toast.error(t("notetaker.live.controlError"));
      }
    } catch (e) {
      console.warn("meeting control invoke failed:", e);
      toast.error(t("notetaker.live.controlError"));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="nt-live" aria-label={t("notetaker.live.label")}>
      <AppLogo meetingId={item.id} app={item.source_app} size="lg" />
      <div className="nt-live-body">
        <button
          type="button"
          className="nt-live-title"
          onClick={() => onOpen(item.id)}
          title={t("notetaker.live.open")}
        >
          <span className="nt-rec" data-paused={paused} aria-hidden="true" />
          <span>{item.title}</span>
        </button>
        <div className="nt-live-meta">
          <span className="nt-live-clock" role="timer">
            {formatClock(seconds)}
          </span>
          {paused ? (
            <span>{t("notetaker.status.paused")}</span>
          ) : (
            <span className="nt-wave" aria-hidden="true">
              {Array.from({ length: WAVE_BARS }, (_, i) => (
                <i key={i} />
              ))}
            </span>
          )}
          <span>
            {item.capture_system_audio
              ? t("notetaker.live.micAndSystem")
              : t("notetaker.live.micOnly")}
          </span>
        </div>
      </div>
      <div className="nt-live-actions">
        <button
          type="button"
          className="icon-button nt-live-pause"
          disabled={busy}
          aria-label={
            paused ? t("notetaker.live.resume") : t("notetaker.live.pause")
          }
          title={
            paused ? t("notetaker.live.resume") : t("notetaker.live.pause")
          }
          onClick={() =>
            void run(() =>
              paused ? commands.meetingResume() : commands.meetingPause(),
            )
          }
        >
          {paused ? (
            <Play width={16} height={16} aria-hidden="true" />
          ) : (
            <Pause width={16} height={16} aria-hidden="true" />
          )}
        </button>
        <button
          type="button"
          className="nt-btn nt-btn-primary nt-btn-sm"
          disabled={busy}
          onClick={() => void run(() => commands.meetingStop())}
        >
          <Square width={13} height={13} aria-hidden="true" />
          {t("notetaker.live.stop")}
        </button>
      </div>
    </section>
  );
};
