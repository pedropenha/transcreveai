import React from "react";
import { CircleDot, Mic, StickyNote } from "lucide-react";
import { hoverTipText, type HoverTipParts } from "./flowbarView";

export type FlowbarTip = "dictate" | "notetaker" | "notes" | "error";

interface FlowbarHoverCardProps {
  dictateTip: HoverTipParts;
  notetakerLabel: string;
  notesLabel: string;
  onTip: (tip: FlowbarTip | null) => void;
  onDictate: () => void;
  onNotetaker: () => void;
  onNotes: () => void;
}

interface HoverButtonProps {
  tip: FlowbarTip;
  label: string;
  primary?: boolean;
  onTip: (tip: FlowbarTip | null) => void;
  onClick: () => void;
  children: React.ReactNode;
}

function HoverButton({
  tip,
  label,
  primary = false,
  onTip,
  onClick,
  children,
}: HoverButtonProps) {
  return (
    <button
      type="button"
      className={`fbtn ${primary ? "fbtn-primary" : "fbtn-round"}`}
      aria-label={label}
      onMouseEnter={() => onTip(tip)}
      onFocus={() => onTip(tip)}
      onMouseLeave={() => onTip(null)}
      onBlur={() => onTip(null)}
      onClick={onClick}
    >
      {children}
    </button>
  );
}

/** Hover state of the Flow Bar: highlighted mic, notetaker dot and notes. */
export function FlowbarHoverCard({
  dictateTip,
  notetakerLabel,
  notesLabel,
  onTip,
  onDictate,
  onNotetaker,
  onNotes,
}: FlowbarHoverCardProps) {
  return (
    <div className="f-hover-group">
      <HoverButton
        tip="dictate"
        label={hoverTipText(dictateTip)}
        primary
        onTip={onTip}
        onClick={onDictate}
      >
        <Mic size={18} aria-hidden="true" />
      </HoverButton>
      <HoverButton
        tip="notetaker"
        label={notetakerLabel}
        onTip={onTip}
        onClick={onNotetaker}
      >
        <CircleDot size={18} aria-hidden="true" />
      </HoverButton>
      <HoverButton
        tip="notes"
        label={notesLabel}
        onTip={onTip}
        onClick={onNotes}
      >
        <StickyNote size={16} aria-hidden="true" />
      </HoverButton>
    </div>
  );
}

/** Tooltip text: label with the shortcut in bold, when there is one. */
export function FlowbarTipText({ label, shortcut }: HoverTipParts) {
  return (
    <span className="fbar-tip-text">
      {label}
      {shortcut !== null && <strong>{shortcut}</strong>}
    </span>
  );
}
