import React from "react";
import { Check, Loader2 } from "lucide-react";

export type SetupTone = "pending" | "busy" | "done" | "warn";

const BUBBLE: Record<SetupTone, string> = {
  pending: "bg-logo-primary/15 text-logo-primary",
  busy: "bg-logo-primary/15 text-logo-primary",
  done: "bg-success-soft text-success",
  warn: "bg-warning-soft text-warning",
};

const BORDER: Record<SetupTone, string> = {
  pending: "border-mid-gray/20",
  busy: "border-mid-gray/20",
  done: "border-mid-gray/20",
  warn: "border-warning/30",
};

interface SetupRowProps {
  tone: SetupTone;
  icon: React.ReactNode;
  title: string;
  badge?: React.ReactNode;
  description?: React.ReactNode;
  children?: React.ReactNode;
}

/**
 * One line of the first-run setup screen — same card anatomy as the
 * permission cards of AccessibilityOnboarding (round icon, title, text,
 * state), with the icon turning into a check once the step is done.
 */
export const SetupRow: React.FC<SetupRowProps> = ({
  tone,
  icon,
  title,
  badge,
  description,
  children,
}) => (
  <section
    className={`w-full p-4 rounded-lg bg-white/5 border ${BORDER[tone]} flex items-start gap-4 text-left`}
    aria-busy={tone === "busy"}
  >
    <div
      className={`w-10 h-10 rounded-full grid place-items-center shrink-0 ${BUBBLE[tone]}`}
      aria-hidden="true"
    >
      {tone === "done" ? (
        <Check className="w-5 h-5" />
      ) : tone === "busy" ? (
        <Loader2 className="w-5 h-5 animate-spin" />
      ) : (
        icon
      )}
    </div>
    <div className="flex-1 min-w-0">
      <h3 className="font-medium text-text flex items-center gap-2 flex-wrap">
        {title}
        {badge}
      </h3>
      {description ? (
        <p className="text-sm text-text/60">{description}</p>
      ) : null}
      {children}
    </div>
  </section>
);

export const StatusLine: React.FC<{
  tone: "ok" | "wait" | "error";
  children: React.ReactNode;
}> = ({ tone, children }) => (
  <p
    role="status"
    className={`mt-2 flex items-center gap-1.5 text-sm ${
      tone === "ok"
        ? "text-success"
        : tone === "error"
          ? "text-error"
          : "text-text/55"
    }`}
  >
    {tone === "ok" ? <Check className="w-4 h-4" aria-hidden="true" /> : null}
    {children}
  </p>
);
