import React from "react";
import ReactDOM from "react-dom/client";
import { platform } from "@tauri-apps/plugin-os";
import { listen } from "@tauri-apps/api/event";
import MeetingWindow from "./MeetingWindow";
import {
  applyTheme,
  getStoredTheme,
  syncThemeFromSettings,
} from "@/lib/utils/theme";
import type { Theme } from "@/bindings";
import "@/i18n";
import "@/App.css";

// Normal decorated window (F009/T-066): same boot sequence as the Hub — the
// shared palette tokens live in App.css, so importing it gives this document
// the themed Tailwind utilities. Theme is applied synchronously to avoid a
// flash, then reconciled with the persisted setting and followed live.
document.documentElement.dataset.platform = platform();
applyTheme(getStoredTheme());
syncThemeFromSettings();
listen<Theme>("theme-changed", (event) => applyTheme(event.payload));

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <MeetingWindow />
  </React.StrictMode>,
);
