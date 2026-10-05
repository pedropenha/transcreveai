import React, { useEffect, useState } from "react";
import type { HistoryEntry } from "@/bindings";
import { resolveAppIcon } from "@/toast/appIcon";
import { monogram } from "../notetaker/notetakerView";
import { useSeen } from "../useSeen";
import { cachedOriginIcon, loadOriginIcon } from "./historyIcons";
import {
  originIconKey,
  originLabel,
  originLogoCandidates,
  type OriginApp,
} from "./homeView";

interface HistoryAppLogoProps {
  entry: Pick<HistoryEntry, "id"> & OriginApp;
  size?: "sm" | "md";
}

/** Embedded logo for a known app, trying the friendly name then the exe stem. */
function bundledLogo(entry: OriginApp): string | null {
  for (const label of originLogoCandidates(entry)) {
    const icon = resolveAppIcon(label);
    if (icon.kind === "img") return icon.src;
  }
  return null;
}

/**
 * Icon of the app a dictation was spoken into: embedded logo for known apps,
 * else the backend-extracted exe icon (lazy per visible row, cached per entry),
 * else a monogram of the app name. Entries without an origin (recorded before
 * it was captured) show a neutral tile. Decorative: the name is always in the
 * adjacent text.
 */
export const HistoryAppLogo: React.FC<HistoryAppLogoProps> = ({
  entry,
  size = "sm",
}) => {
  const [ref, seen] = useSeen<HTMLSpanElement>();
  const bundled = bundledLogo(entry);
  const key = originIconKey(entry);
  // Keyed by entry so a reused instance never shows another row's icon.
  const [loaded, setLoaded] = useState<{
    key: string;
    icon: string | null;
  } | null>(null);

  useEffect(() => {
    if (!seen || bundled !== null || key === null) return;
    let cancelled = false;
    void loadOriginIcon(key, entry.id).then((icon) => {
      if (!cancelled) setLoaded({ key, icon });
    });
    return () => {
      cancelled = true;
    };
  }, [seen, bundled, key, entry.id]);

  const extracted =
    key === null
      ? null
      : (cachedOriginIcon(key) ?? (loaded?.key === key ? loaded.icon : null));
  const src = bundled ?? extracted;
  const label = originLabel(entry);
  const mono = monogram(label);
  const kind = src ? "img" : label === null ? "unknown" : "monogram";

  return (
    <span
      ref={ref}
      className={`hist-applogo${size === "md" ? " hist-applogo-md" : ""}`}
      data-kind={kind}
      style={
        kind === "monogram"
          ? ({ "--mono-hue": mono.hue } as React.CSSProperties)
          : undefined
      }
      aria-hidden="true"
    >
      {kind === "img" && src ? (
        <img src={src} alt="" draggable={false} />
      ) : kind === "unknown" ? (
        "?"
      ) : (
        mono.letter
      )}
    </span>
  );
};
