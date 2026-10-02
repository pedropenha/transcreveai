import React, { useEffect, useId, useRef, useState } from "react";
import { ChevronDown } from "lucide-react";
import { useTranslation } from "react-i18next";

interface StartMeetingMenuProps {
  onStart: (micOnly: boolean) => void;
  disabled?: boolean;
}

const OPTIONS: ReadonlyArray<{ labelKey: string; micOnly: boolean }> = [
  { labelKey: "settings.meetings.newMeetingCall", micOnly: false },
  { labelKey: "settings.meetings.newMeetingInPerson", micOnly: true },
];

/**
 * FR-009-01: "Iniciar Notetaker" — Chamada no computador (mic + system) or
 * Presencial (mic only). Menu pattern: arrows/Home/End move, Esc closes and
 * returns focus to the trigger.
 */
export const StartMeetingMenu: React.FC<StartMeetingMenuProps> = ({
  onStart,
  disabled = false,
}) => {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const menuId = useId();
  const triggerId = `${menuId}-trigger`;
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const handleClickOutside = (event: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(event.target as Node)) {
        setOpen(false);
      }
    };
    document.addEventListener("mousedown", handleClickOutside);
    rootRef.current
      ?.querySelector<HTMLButtonElement>('[role="menuitem"]')
      ?.focus();
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, [open]);

  const onMenuKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    const items = Array.from(
      event.currentTarget.querySelectorAll<HTMLButtonElement>(
        '[role="menuitem"]',
      ),
    );
    if (event.key === "Escape") {
      event.preventDefault();
      setOpen(false);
      document.getElementById(triggerId)?.focus();
      return;
    }
    const index = items.indexOf(event.target as HTMLButtonElement);
    if (index < 0) return;
    const last = items.length - 1;
    const next =
      event.key === "ArrowDown"
        ? (index + 1) % items.length
        : event.key === "ArrowUp"
          ? (index - 1 + items.length) % items.length
          : event.key === "Home"
            ? 0
            : event.key === "End"
              ? last
              : -1;
    if (next >= 0) {
      event.preventDefault();
      items[next]?.focus();
    }
  };

  return (
    <div className="nt-start" ref={rootRef}>
      <button
        id={triggerId}
        type="button"
        className="nt-btn nt-btn-rec"
        disabled={disabled}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={open ? menuId : undefined}
        onClick={() => setOpen((value) => !value)}
      >
        <span className="nt-btn-dot" aria-hidden="true" />
        {t("notetaker.start")}
        <ChevronDown width={14} height={14} aria-hidden="true" />
      </button>
      {open ? (
        <div
          id={menuId}
          role="menu"
          aria-labelledby={triggerId}
          className="nt-menu"
          onKeyDown={onMenuKeyDown}
          onBlur={(event) => {
            if (!event.currentTarget.contains(event.relatedTarget as Node)) {
              setOpen(false);
            }
          }}
        >
          {OPTIONS.map((option) => (
            <button
              key={option.labelKey}
              type="button"
              role="menuitem"
              onClick={() => {
                setOpen(false);
                onStart(option.micOnly);
              }}
            >
              {t(option.labelKey)}
            </button>
          ))}
        </div>
      ) : null}
    </div>
  );
};
