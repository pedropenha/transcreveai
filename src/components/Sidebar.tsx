import React from "react";
import {
  BookOpen,
  CircleQuestionMark,
  Home,
  PanelLeftClose,
  PanelLeftOpen,
  Radio,
  Settings,
  Sparkles,
  StickyNote,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import SoundBarsIcon from "./icons/SoundBarsIcon";
import { HelpPage } from "./help/HelpPage";
import { HomePage } from "./home/HomePage";
import { DictionarySettings, MeetingsSettings, SettingsHub } from "./settings";
import { SetupChecklist } from "./shell/SetupChecklist";
import {
  RAIL_ENTRIES,
  type HubSection,
  type Navigation,
  type RailEntry,
  type RailGroup,
} from "./shell/navModel";
import "./shell/shell.css";

export type SidebarSection = HubSection;

interface IconProps {
  width?: number | string;
  height?: number | string;
  className?: string;
}

export const SECTIONS_CONFIG: Record<
  HubSection,
  { component: React.ComponentType }
> = {
  home: { component: HomePage },
  meetings: { component: MeetingsSettings },
  dictionary: { component: DictionarySettings },
  settings: { component: SettingsHub },
  help: { component: HelpPage },
};

const ICONS: Record<string, React.ComponentType<IconProps>> = {
  home: Home,
  meetings: Radio,
  notes: StickyNote,
  dictionary: BookOpen,
  assistant: Sparkles,
  settings: Settings,
  help: CircleQuestionMark,
};

interface SidebarProps {
  activeSection: SidebarSection;
  onSectionChange: (section: SidebarSection) => void;
  onNavigate: (navigation: Navigation) => void;
  collapsed: boolean;
  onToggleCollapsed: () => void;
}

const entriesOf = (group: RailGroup) =>
  RAIL_ENTRIES.filter((entry) => entry.group === group);

export const Sidebar: React.FC<SidebarProps> = ({
  activeSection,
  onSectionChange,
  onNavigate,
  collapsed,
  onToggleCollapsed,
}) => {
  const { t } = useTranslation();

  const renderEntry = (entry: RailEntry) => {
    const Icon = ICONS[entry.id];
    const label = t(entry.labelKey);
    if (entry.availability === "soon" || !entry.section) {
      return (
        <button
          key={entry.id}
          type="button"
          className="rail-item"
          aria-disabled="true"
          title={`${label} · ${t("sidebar.soonHint")}`}
          onClick={(event) => event.preventDefault()}
        >
          <Icon width={18} height={18} aria-hidden="true" />
          <span className="rail-label-text">{label}</span>
          <span className="rail-tag">{t("sidebar.soonTag")}</span>
        </button>
      );
    }
    const section = entry.section;
    return (
      <button
        key={entry.id}
        type="button"
        className="rail-item"
        aria-current={activeSection === section ? "page" : undefined}
        title={label}
        onClick={() => onSectionChange(section)}
      >
        <Icon width={18} height={18} aria-hidden="true" />
        <span className="rail-label-text">{label}</span>
      </button>
    );
  };

  return (
    <nav
      className="hub-rail"
      data-collapsed={collapsed}
      aria-label={t("sidebar.label")}
    >
      <div className="rail-logo">
        <button
          type="button"
          className="rail-brand"
          aria-label={t("sidebar.home")}
          onClick={() => onSectionChange("home")}
        >
          <span className="rail-mark">
            <SoundBarsIcon width={16} height={16} aria-hidden="true" />
          </span>
          <span className="rail-wordmark">
            {t("sidebar.wordmarkBase")}
            <em>{t("sidebar.wordmarkSuffix")}</em>
          </span>
        </button>
        <button
          type="button"
          className="icon-button rail-collapse"
          aria-label={t(collapsed ? "sidebar.expand" : "sidebar.collapse")}
          aria-expanded={!collapsed}
          title={t(collapsed ? "sidebar.expand" : "sidebar.collapse")}
          onClick={onToggleCollapsed}
        >
          {collapsed ? (
            <PanelLeftOpen width={16} height={16} aria-hidden="true" />
          ) : (
            <PanelLeftClose width={16} height={16} aria-hidden="true" />
          )}
        </button>
      </div>
      <div className="rail-group">{entriesOf("main").map(renderEntry)}</div>
      <span className="caps rail-group-label">
        {t("sidebar.groupCustomize")}
      </span>
      <div className="rail-group">
        {entriesOf("customize").map(renderEntry)}
      </div>
      <div className="rail-spacer" />
      {collapsed ? null : <SetupChecklist onNavigate={onNavigate} />}
      <div className="rail-foot">{entriesOf("footer").map(renderEntry)}</div>
    </nav>
  );
};
