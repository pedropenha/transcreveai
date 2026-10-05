import React, { useEffect, useState } from "react";
import type { SourceApp } from "@/bindings";
import { resolveAppIcon } from "@/toast/appIcon";
import { useSeen } from "../useSeen";
import { cachedSourceIcon, loadSourceIcon } from "./sourceIcons";
import { iconCacheKey, logoCandidates, monogram } from "./notetakerView";

interface AppLogoProps {
  meetingId: string;
  app: SourceApp | null;
  size?: "md" | "lg";
}

/** Embedded logo for a known app, trying the friendly name then the exe stem. */
function bundledLogo(app: SourceApp | null): string | null {
  for (const label of logoCandidates(app)) {
    const icon = resolveAppIcon(label);
    if (icon.kind === "img") return icon.src;
  }
  return null;
}

/**
 * Logo of the app a meeting came from: embedded logo for known apps, else the
 * backend-extracted exe icon (lazy, cached per exe), else a monogram. Manual /
 * in-person meetings (no source app) show the Transcreve.ai mark. Decorative:
 * the app name is always in the row text.
 */
export const AppLogo: React.FC<AppLogoProps> = ({
  meetingId,
  app,
  size = "md",
}) => {
  const [ref, seen] = useSeen<HTMLSpanElement>();
  const bundled = bundledLogo(app);
  const key = iconCacheKey(app);
  // Keyed by exe so a reused instance never shows another app's icon.
  const [loaded, setLoaded] = useState<{
    key: string;
    icon: string | null;
  } | null>(null);

  useEffect(() => {
    if (!seen || bundled !== null || key === null) return;
    let cancelled = false;
    void loadSourceIcon(key, meetingId).then((icon) => {
      if (!cancelled) setLoaded({ key, icon });
    });
    return () => {
      cancelled = true;
    };
  }, [seen, bundled, key, meetingId]);

  const extracted =
    key === null
      ? null
      : (cachedSourceIcon(key) ?? (loaded?.key === key ? loaded.icon : null));
  const src = bundled ?? extracted;
  const mono = monogram(app?.name);
  const kind = src ? "img" : app === null ? "brand" : "monogram";

  return (
    <span
      ref={ref}
      className={`applogo${size === "lg" ? " applogo-lg" : ""}${
        kind === "brand" ? " applogo-brand" : ""
      }${kind === "monogram" ? " applogo-mono" : ""}`}
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
      ) : kind === "brand" ? (
        <svg viewBox="0 0 24 24" fill="currentColor">
          <rect x="3.5" y="9" width="2.4" height="6" rx="1.2" />
          <rect x="7.5" y="5.5" width="2.4" height="13" rx="1.2" />
          <rect x="11.5" y="8" width="2.4" height="8" rx="1.2" />
          <rect x="15.5" y="11" width="2.4" height="2" rx="1" />
        </svg>
      ) : (
        mono.letter
      )}
    </span>
  );
};
