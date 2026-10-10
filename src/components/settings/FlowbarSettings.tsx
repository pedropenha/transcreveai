import React from "react";
import { useTranslation } from "react-i18next";
import { Dropdown } from "../ui/Dropdown";
import { SettingContainer } from "../ui/SettingContainer";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { useSettings } from "../../hooks/useSettings";
import type {
  FlowbarEdge,
  FlowbarFollow,
  FlowbarVisibility,
  OverlayStyle,
} from "@/bindings";

interface FlowbarSettingsProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

export const FlowbarSettings: React.FC<FlowbarSettingsProps> = React.memo(
  ({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();

    const visibilityOptions = [
      {
        value: "always",
        label: t("settings.flowbar.visibility.options.always"),
      },
      {
        value: "during_recording",
        label: t("settings.flowbar.visibility.options.during_recording"),
      },
      {
        value: "never",
        label: t("settings.flowbar.visibility.options.never"),
      },
    ];

    const followOptions = [
      {
        value: "foreground_monitor",
        label: t("settings.flowbar.follow.options.foreground_monitor"),
      },
      {
        value: "cursor",
        label: t("settings.flowbar.follow.options.cursor"),
      },
      {
        value: "primary_monitor",
        label: t("settings.flowbar.follow.options.primary_monitor"),
      },
    ];

    const edgeOptions = [
      {
        value: "bottom",
        label: t("settings.flowbar.edge.options.bottom"),
      },
      {
        value: "left",
        label: t("settings.flowbar.edge.options.left"),
      },
      {
        value: "right",
        label: t("settings.flowbar.edge.options.right"),
      },
    ];

    const selectedVisibility = (getSetting("flowbar_visibility") ||
      "always") as FlowbarVisibility;
    const selectedFollow = (getSetting("flowbar_follow") ||
      "foreground_monitor") as FlowbarFollow;
    const selectedEdge = (getSetting("flowbar_position_edge") ||
      "bottom") as FlowbarEdge;
    const showNotetaker = getSetting("flowbar_show_notetaker") ?? true;
    const showNotes = getSetting("flowbar_show_notes") ?? false;
    const overlayStyle = (getSetting("overlay_style") ||
      "live") as OverlayStyle;

    // Placement and button controls only matter while a bar can be on screen:
    // hide them when the Flow Bar is off entirely (or the recording overlay is
    // set to "none", matching ShowOverlay's conditional).
    const barConfigurable =
      selectedVisibility !== "never" && overlayStyle !== "none";

    return (
      <>
        <SettingContainer
          title={t("settings.flowbar.visibility.title")}
          description={t("settings.flowbar.visibility.description")}
          descriptionMode={descriptionMode}
          grouped={grouped}
        >
          <Dropdown
            options={visibilityOptions}
            selectedValue={selectedVisibility}
            onSelect={(value) =>
              updateSetting("flowbar_visibility", value as FlowbarVisibility)
            }
            disabled={isUpdating("flowbar_visibility")}
          />
        </SettingContainer>

        {barConfigurable && (
          <>
            <SettingContainer
              title={t("settings.flowbar.follow.title")}
              description={t("settings.flowbar.follow.description")}
              descriptionMode={descriptionMode}
              grouped={grouped}
            >
              <Dropdown
                options={followOptions}
                selectedValue={selectedFollow}
                onSelect={(value) =>
                  updateSetting("flowbar_follow", value as FlowbarFollow)
                }
                disabled={isUpdating("flowbar_follow")}
              />
            </SettingContainer>

            <SettingContainer
              title={t("settings.flowbar.edge.title")}
              description={t("settings.flowbar.edge.description")}
              descriptionMode={descriptionMode}
              grouped={grouped}
            >
              <Dropdown
                options={edgeOptions}
                selectedValue={selectedEdge}
                onSelect={(value) =>
                  updateSetting("flowbar_position_edge", value as FlowbarEdge)
                }
                disabled={isUpdating("flowbar_position_edge")}
              />
            </SettingContainer>

            <ToggleSwitch
              checked={showNotetaker}
              onChange={(enabled) =>
                updateSetting("flowbar_show_notetaker", enabled)
              }
              isUpdating={isUpdating("flowbar_show_notetaker")}
              label={t("settings.flowbar.showNotetaker.label")}
              description={t("settings.flowbar.showNotetaker.description")}
              descriptionMode={descriptionMode}
              grouped={grouped}
            />

            <ToggleSwitch
              checked={showNotes}
              onChange={(enabled) =>
                updateSetting("flowbar_show_notes", enabled)
              }
              isUpdating={isUpdating("flowbar_show_notes")}
              label={t("settings.flowbar.showNotes.label")}
              description={t("settings.flowbar.showNotes.description")}
              descriptionMode={descriptionMode}
              grouped={grouped}
            />
          </>
        )}
      </>
    );
  },
);
