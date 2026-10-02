import React, { useEffect, useRef, useState } from "react";
import {
  ChevronRight,
  Cpu,
  FileText,
  Globe,
  Keyboard,
  KeyRound,
  Mic,
  Monitor,
  Settings as SettingsIcon,
  ShieldCheck,
  SlidersHorizontal,
  Sparkles,
  type LucideIcon,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import {
  onSettingsTabNavigate,
  takePendingSettingsTab,
} from "./pendingSettingsTab";
import { SettingsPageContent } from "./SettingsPages";
import {
  DEFAULT_SETTINGS_PAGE,
  SETTINGS_CATEGORIES,
  categoryOf,
  pagesOf,
  type SettingsPage,
} from "./settingsNav";
import "./settings.css";

const PAGE_ICONS: Record<SettingsPage, LucideIcon> = {
  "usage/general": SlidersHorizontal,
  "usage/shortcuts": Keyboard,
  "usage/audio": Mic,
  "transcription/models": Cpu,
  "transcription/languages": Globe,
  "transcription/api": KeyRound,
  "intelligence/summaries": FileText,
  "intelligence/assistant": Sparkles,
  "app/system": Monitor,
  "app/privacy": ShieldCheck,
  "app/advanced": SettingsIcon,
};

/**
 * Configurações (F010 FR-010-27): sub-navigation by category on the left, one
 * page on the right. Deep links (`pendingSettingsTab`) accept old flat names
 * and `category/page` paths.
 */
export const SettingsHub: React.FC = () => {
  const { t } = useTranslation();
  const [page, setPage] = useState<SettingsPage>(DEFAULT_SETTINGS_PAGE);
  const scrollRef = useRef<HTMLElement>(null);
  const headingRef = useRef<HTMLHeadingElement>(null);
  const firstPage = useRef(true);

  // The navigation event that mounts this hub fires before any listener here
  // exists, so `pendingSettingsTab` stashes it and the first effect drains it.
  useEffect(() => {
    const stashed = takePendingSettingsTab();
    if (stashed) setPage(stashed);
    return onSettingsTabNavigate(setPage);
  }, []);

  // A page change is announced by moving focus to the new page title (not on
  // the very first render, which would steal focus from the Hub).
  useEffect(() => {
    scrollRef.current?.scrollTo({ top: 0 });
    if (firstPage.current) {
      firstPage.current = false;
      return;
    }
    headingRef.current?.focus({ preventScroll: true });
  }, [page]);

  const category = categoryOf(page);
  const slug = page.split("/")[1] as string;

  return (
    <div className="st-root">
      <nav className="st-subnav" aria-label={t("settingsHub.sectionsLabel")}>
        <p className="st-subnav-title">{t("settingsHub.title")}</p>
        {SETTINGS_CATEGORIES.map((cat) => (
          <div key={cat} className="st-subnav-group">
            <span className="caps">{t(`settingsHub.categories.${cat}`)}</span>
            {pagesOf(cat).map((entry) => {
              const Icon = PAGE_ICONS[entry.id];
              return (
                <button
                  key={entry.id}
                  type="button"
                  className="st-subnav-item"
                  aria-current={page === entry.id ? "page" : undefined}
                  onClick={() => setPage(entry.id)}
                >
                  <Icon width={16} height={16} aria-hidden="true" />
                  {t(`settingsHub.pages.${entry.slug}.title`)}
                </button>
              );
            })}
          </div>
        ))}
      </nav>
      <main className="st-main" ref={scrollRef}>
        <div className="st-crumb">
          {t("settingsHub.title")}
          <ChevronRight width={13} height={13} aria-hidden="true" />
          {t(`settingsHub.categories.${category}`)}
        </div>
        <header className="st-page-head">
          <h1 ref={headingRef} tabIndex={-1}>
            {t(`settingsHub.pages.${slug}.heading`, {
              defaultValue: t(`settingsHub.pages.${slug}.title`),
            })}
          </h1>
          <p>{t(`settingsHub.pages.${slug}.description`)}</p>
        </header>
        <SettingsPageContent page={page} />
      </main>
    </div>
  );
};
