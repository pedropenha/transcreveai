import React from "react";
import { useTranslation } from "react-i18next";
import { ChevronDown, X } from "lucide-react";
import SoundBarsIcon from "@/components/icons/SoundBarsIcon";
import AppLogo from "./AppLogo";
import {
  meetingMenuItems,
  type DetectorAction,
  type MeetingDetection,
} from "./toastView";

/** Props spread onto a button so the action runs once per gesture
 *  (pointerdown + click dedupe lives in ToastOverlay). */
type PressableProps = React.HTMLAttributes<HTMLButtonElement>;
export type Pressable = (id: string, action: () => void) => PressableProps;

interface MeetingCardProps {
  detection: MeetingDetection | null;
  menuOpen: boolean;
  cardRef: React.RefObject<HTMLDivElement>;
  menuRef: React.RefObject<HTMLDivElement>;
  menuButtonRef: React.RefObject<HTMLButtonElement>;
  pressable: Pressable;
  onRespond: (action: DetectorAction) => void;
  onToggleMenu: () => void;
  onHoverChange: (hovered: boolean) => void;
  onPointerMove: () => void;
  onMenuKeyDown: (e: React.KeyboardEvent) => void;
}

/** FR-008-07/08: the always-expanded "Reunião detectada" card — app icon,
 *  title + "● Agora · <App>", cream "Iniciar Notetaker" split button whose
 *  chevron opens the action menu; ✕ shows on hover/focus. */
const MeetingCard: React.FC<MeetingCardProps> = ({
  detection,
  menuOpen,
  cardRef,
  menuRef,
  menuButtonRef,
  pressable,
  onRespond,
  onToggleMenu,
  onHoverChange,
  onPointerMove,
  onMenuKeyDown,
}) => {
  const { t } = useTranslation();
  const app = detection?.app_label ?? "";
  return (
    <div
      ref={cardRef}
      className={`tcard tmeeting ${menuOpen ? "menu-open" : ""}`}
      onMouseEnter={() => onHoverChange(true)}
      onMouseLeave={() => onHoverChange(false)}
      onPointerMove={onPointerMove}
      // Menu arrows live on the card, not the menu: a mouse-opened menu
      // on an unfocused webview can leave focus on the trigger — events
      // there never reach a `.tmenu`-level handler.
      onKeyDown={menuOpen ? onMenuKeyDown : undefined}
    >
      <div className="trow">
        <AppLogo label={app} icon={detection?.icon} />
        <div className="ttexts">
          <span className="ttitle">{t("toast.meetingDetected")}</span>
          <span className="tsub" title={app}>
            <i className="tnow" aria-hidden="true" />
            <span className="tsub-text">{t("toast.nowWithApp", { app })}</span>
          </span>
        </div>
        <div className="tsplit">
          <button
            type="button"
            className="tsplit-main"
            {...pressable("meeting-start", () => onRespond("start"))}
          >
            <SoundBarsIcon className="tsplit-icon" width={16} height={16} />
            {t("toast.startNotetaker")}
          </button>
          <button
            type="button"
            ref={menuButtonRef}
            className="tsplit-chevron"
            aria-label={t("toast.moreActions")}
            aria-expanded={menuOpen}
            aria-haspopup="menu"
            aria-controls="toast-meeting-menu"
            {...pressable("meeting-menu", onToggleMenu)}
          >
            <ChevronDown size={16} aria-hidden="true" />
          </button>
        </div>
        <button
          type="button"
          className="tdismiss"
          aria-label={t("toast.dismiss")}
          {...pressable("meeting-close", () => onRespond("dismiss"))}
        >
          <X size={10} aria-hidden="true" />
        </button>
      </div>
      {menuOpen && (
        <div
          ref={menuRef}
          id="toast-meeting-menu"
          className="tmenu"
          role="menu"
        >
          {meetingMenuItems().map((item) => (
            <button
              key={item.action}
              type="button"
              role="menuitem"
              className="tmenu-item"
              {...pressable(`menu-${item.action}`, () =>
                onRespond(item.action),
              )}
            >
              {t(`toast.${item.labelKey}`, { app })}
            </button>
          ))}
        </div>
      )}
    </div>
  );
};

export default MeetingCard;
