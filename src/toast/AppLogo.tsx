import React from "react";
import { Video } from "lucide-react";
import { resolveAppIcon } from "./appIcon";

interface AppLogoProps {
  label: string;
  icon?: string | null;
}

/** Rounded app logo for the detection card: backend PNG, bundled logo, or a
 *  neutral video glyph when neither exists. Decorative — the title and the
 *  app label already carry the meaning. */
const AppLogo: React.FC<AppLogoProps> = ({ label, icon }) => {
  const resolved = resolveAppIcon(label, icon);
  if (resolved.kind === "img") {
    return (
      <img
        className="tapp-icon"
        src={resolved.src}
        alt=""
        width={32}
        height={32}
        draggable={false}
      />
    );
  }
  return (
    <span className="tapp-icon tapp-icon-fallback" aria-hidden="true">
      <Video size={18} />
    </span>
  );
};

export default AppLogo;
