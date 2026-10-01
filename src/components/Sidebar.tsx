import React from "react";
import { BookOpen, Cpu, Home, Settings, Video } from "lucide-react";
import { useTranslation } from "react-i18next";
import SoundBarsIcon from "./icons/SoundBarsIcon";
import {
  DictionarySettings,
  HistorySettings,
  MeetingsSettings,
  ModelsSettings,
  SettingsHub,
} from "./settings";

export type SidebarSection = keyof typeof SECTIONS_CONFIG;

interface IconProps {
  width?: number | string;
  height?: number | string;
  className?: string;
}

interface SectionConfig {
  labelKey: string;
  icon: React.ComponentType<IconProps>;
  component: React.ComponentType;
}

export const SECTIONS_CONFIG = {
  home: {
    labelKey: "sidebar.home",
    icon: Home,
    component: HistorySettings,
  },
  meetings: {
    labelKey: "sidebar.meetings",
    icon: Video,
    component: MeetingsSettings,
  },
  dictionary: {
    labelKey: "sidebar.dictionary",
    icon: BookOpen,
    component: DictionarySettings,
  },
  models: {
    labelKey: "sidebar.models",
    icon: Cpu,
    component: ModelsSettings,
  },
  settings: {
    labelKey: "sidebar.settings",
    icon: Settings,
    component: SettingsHub,
  },
} as const satisfies Record<string, SectionConfig>;

interface SidebarProps {
  activeSection: SidebarSection;
  onSectionChange: (section: SidebarSection) => void;
}

export const Sidebar: React.FC<SidebarProps> = ({
  activeSection,
  onSectionChange,
}) => {
  const { t } = useTranslation();
  const availableSections = Object.entries(SECTIONS_CONFIG).map(
    ([id, config]) => ({
      id: id as SidebarSection,
      ...config,
    }),
  );

  return (
    <nav className="hub-rail" aria-label={t("sidebar.label")}>
      <div className="hub-rail-title">
        <SoundBarsIcon width={18} height={18} aria-hidden="true" />
        <span>{t("sidebar.productName")}</span>
      </div>
      <div className="hub-rail-list">
        {availableSections.map((section) => {
          const Icon = section.icon;
          const active = activeSection === section.id;
          return (
            <button
              key={section.id}
              type="button"
              className="hub-rail-item"
              aria-current={active ? "page" : undefined}
              title={t(section.labelKey)}
              onClick={() => onSectionChange(section.id)}
            >
              <Icon width={18} height={18} aria-hidden="true" />
              <span>{t(section.labelKey)}</span>
            </button>
          );
        })}
      </div>
    </nav>
  );
};
