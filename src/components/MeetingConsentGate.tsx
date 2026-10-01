import React, { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { commands } from "@/bindings";
import { Button } from "./ui/Button";
import { Dialog } from "./ui";

// `toast://show` payload (contracts.md §5) — emitted via `app.emit`, so it is
// not part of the specta event map; the shape is declared here.
interface ToastShowPayload {
  kind: string;
  message: string;
  action?: string;
}

// FR-009-02: first-use consent modal. Every meeting start refused with
// `consent_required` surfaces as `toast://show` {kind: "meeting_consent",
// action: "open_consent"} (the compact reminder toast on each successful
// start uses the same kind but `action: "copy_consent"` — that one belongs
// to the T-062 toast window, not to this modal).
const MeetingConsentGate: React.FC = () => {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [accepting, setAccepting] = useState(false);

  useEffect(() => {
    const unlisten = listen<ToastShowPayload>("toast://show", (event) => {
      const { kind, action } = event.payload;
      if (kind === "meeting_consent" && action === "open_consent") {
        setOpen(true);
      }
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  // Load the configurable notice text each time the modal opens so the user
  // sees exactly what "Copiar aviso" would paste into the chat.
  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    void commands.meetingConsentCopy().then((result) => {
      if (!cancelled && result.status === "ok") {
        setNotice(result.data);
      }
    });
    return () => {
      cancelled = true;
    };
  }, [open]);

  const copyNotice = async () => {
    const result = await commands.meetingConsentCopy();
    if (result.status !== "ok") return;
    try {
      await navigator.clipboard.writeText(result.data);
      toast.success(t("meeting.consent.copied"));
    } catch (e) {
      console.warn("Failed to copy the meeting notice:", e);
      toast.error(t("meeting.consent.copyFailed"));
    }
  };

  const accept = async () => {
    setAccepting(true);
    try {
      const result = await commands.meetingConsentAccept();
      if (result.status === "ok") {
        setOpen(false);
        toast.success(t("meeting.consent.acceptedHint"));
      }
    } finally {
      setAccepting(false);
    }
  };

  return (
    <Dialog
      open={open}
      title={t("meeting.consent.title")}
      closeLabel={t("common.close")}
      onOpenChange={setOpen}
      footer={
        <>
          <Button variant="secondary" size="md" onClick={copyNotice}>
            {t("meeting.consent.copy")}
          </Button>
          <Button
            variant="primary-soft"
            size="md"
            onClick={accept}
            disabled={accepting}
          >
            {t("meeting.consent.accept")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-3 text-sm text-text">
        <p className="leading-relaxed">{t("meeting.consent.body")}</p>
        {notice && (
          <div>
            <p className="mb-1 text-xs font-medium text-mid-gray">
              {t("meeting.consent.noticeLabel")}
            </p>
            <p className="rounded-md border border-mid-gray/20 bg-mid-gray/10 px-3 py-2 text-text/80">
              {notice}
            </p>
          </div>
        )}
      </div>
    </Dialog>
  );
};

export default MeetingConsentGate;
