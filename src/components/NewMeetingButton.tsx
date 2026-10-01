import React, { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { ChevronDown, MonitorPlay, User } from "lucide-react";
import { commands } from "@/bindings";

/**
 * Hub → "Nova reunião" (FR-009-01): a manual start is the one entry point
 * that also opens the meeting window — detection toast/auto-start confirm
 * in place instead (the Flow Bar pill is their persistent indicator).
 * The split menu carries the two start modes: "Chamada no computador"
 * (mic + system audio) and "Presencial" (mic only).
 *
 * Consent: a `consent_required` refusal already surfaces as the consent
 * modal via `toast://show` (MeetingConsentGate) — no extra toast here.
 */
const NewMeetingButton: React.FC = () => {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [starting, setStarting] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const handleClickOutside = (event: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(event.target as Node)) {
        setOpen(false);
      }
    };
    const handleEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", handleClickOutside);
    document.addEventListener("keydown", handleEscape);
    return () => {
      document.removeEventListener("mousedown", handleClickOutside);
      document.removeEventListener("keydown", handleEscape);
    };
  }, [open]);

  const start = async (micOnly: boolean) => {
    setOpen(false);
    setStarting(true);
    try {
      const result = await commands.meetingStart(micOnly);
      if (result.status === "ok") {
        await commands.meetingWindowOpen(result.data.id);
      } else if (result.error.code !== "consent_required") {
        toast.error(t("meeting.startFailed"), {
          description: result.error.message,
        });
      }
    } finally {
      setStarting(false);
    }
  };

  const options = [
    {
      value: "call",
      micOnly: false,
      icon: <MonitorPlay size={14} aria-hidden="true" />,
      label: t("meeting.startCall"),
      hint: t("meeting.startCallHint"),
    },
    {
      value: "in_person",
      micOnly: true,
      icon: <User size={14} aria-hidden="true" />,
      label: t("meeting.startInPerson"),
      hint: t("meeting.startInPersonHint"),
    },
  ];

  return (
    <div ref={rootRef} className="relative w-full px-1 pb-1">
      <button
        type="button"
        className="flex w-full items-center justify-between gap-2 rounded-lg bg-logo-primary px-3 py-2 text-sm font-medium text-white transition-colors hover:bg-logo-primary/85 disabled:opacity-50"
        aria-label={t("meeting.newOptions")}
        aria-expanded={open}
        aria-haspopup="menu"
        disabled={starting}
        onClick={() => setOpen((v) => !v)}
      >
        <span className="truncate">{t("meeting.new")}</span>
        <ChevronDown
          size={14}
          aria-hidden="true"
          className={`shrink-0 transition-transform ${open ? "rotate-180" : ""}`}
        />
      </button>
      {open && (
        <div
          role="menu"
          aria-label={t("meeting.newOptions")}
          className="absolute inset-x-1 top-full z-50 mt-1 rounded-md border border-mid-gray/30 bg-background shadow-lg"
        >
          {options.map((option) => (
            <button
              key={option.value}
              type="button"
              role="menuitem"
              className="flex w-full items-start gap-2 px-3 py-2 text-start text-sm hover:bg-logo-primary/10"
              onClick={() => void start(option.micOnly)}
            >
              <span className="mt-0.5 shrink-0 text-mid-gray">
                {option.icon}
              </span>
              <span className="min-w-0">
                <span className="block font-medium">{option.label}</span>
                <span className="block text-xs text-mid-gray">
                  {option.hint}
                </span>
              </span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
};

export default NewMeetingButton;
