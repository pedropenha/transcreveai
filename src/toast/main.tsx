import React from "react";
import ReactDOM from "react-dom/client";
import { listen } from "@tauri-apps/api/event";
import ToastOverlay from "./ToastOverlay";
import {
  applyTheme,
  getStoredTheme,
  syncThemeFromSettings,
} from "@/lib/utils/theme";
import type { Theme } from "@/bindings";
import "@/i18n";

// Same bootstrap as the Flow Bar webview: a separate document needs its own
// `data-theme` — last-known theme before render (shared localStorage) to
// avoid a flash, reconcile with the persisted setting, then follow changes.
applyTheme(getStoredTheme());
syncThemeFromSettings();
listen<Theme>("theme-changed", (event) => applyTheme(event.payload));

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ToastOverlay />
  </React.StrictMode>,
);
