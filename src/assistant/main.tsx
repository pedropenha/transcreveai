import React from "react";
import ReactDOM from "react-dom/client";
import { listen } from "@tauri-apps/api/event";
import AssistantPanel from "./AssistantPanel";
import {
  applyTheme,
  getStoredTheme,
  syncThemeFromSettings,
} from "@/lib/utils/theme";
import { installCompatShims } from "@/lib/compat";
import type { Theme } from "@/bindings";
import "@/i18n";

// react-markdown's Object.hasOwn shim (same as the Hub bootstrap).
installCompatShims();

// Same bootstrap as the Flow Bar/toast webviews: a separate document needs
// its own `data-theme` — last-known theme before render (shared
// localStorage) to avoid a flash, reconcile with the persisted setting,
// then follow live changes.
applyTheme(getStoredTheme());
syncThemeFromSettings();
listen<Theme>("theme-changed", (event) => applyTheme(event.payload));

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <AssistantPanel />
  </React.StrictMode>,
);
