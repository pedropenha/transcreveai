import React from "react";
import { useTranslation } from "react-i18next";
import { AboutSettings } from "../settings";

const SHORTCUTS = [
  { keys: ["Ctrl", "1"], labelKey: "help.shortcuts.home" },
  { keys: ["Ctrl", "2"], labelKey: "help.shortcuts.meetings" },
  { keys: ["Ctrl", "3"], labelKey: "help.shortcuts.dictionary" },
  { keys: ["Ctrl", "4"], labelKey: "help.shortcuts.settings" },
  { keys: ["Ctrl", "5"], labelKey: "help.shortcuts.help" },
  { keys: ["Ctrl", ","], labelKey: "help.shortcuts.settingsAlt" },
] as const;

export const HelpPage: React.FC = () => {
  const { t } = useTranslation();
  return (
    <main className="settings-hub">
      <header className="section-page-header">
        <h1>{t("help.title")}</h1>
        <p>{t("help.subtitle")}</p>
      </header>
      <section className="help-shortcuts" aria-labelledby="help-shortcuts">
        <h2 id="help-shortcuts" className="caps">
          {t("help.shortcutsTitle")}
        </h2>
        <ul>
          {SHORTCUTS.map((shortcut) => (
            <li key={shortcut.labelKey}>
              <span>{t(shortcut.labelKey)}</span>
              <span className="help-keys">
                {shortcut.keys.map((key) => (
                  <kbd key={key} className="kbd">
                    {key}
                  </kbd>
                ))}
              </span>
            </li>
          ))}
        </ul>
      </section>
      <AboutSettings />
    </main>
  );
};
