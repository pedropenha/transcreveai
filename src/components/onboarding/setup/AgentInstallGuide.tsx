import React, { useState } from "react";
import { useTranslation } from "react-i18next";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Loader2 } from "lucide-react";
import { Button } from "../../ui/Button";
import { INSTALL_GUIDES, type SetupAgentId } from "../setupFlow";

interface AgentInstallGuideProps {
  agent: SetupAgentId;
  onCheck: () => void;
  onBack: () => void;
}

/**
 * Official install command (copyable) + guide link for one agent. The app
 * never runs the installer itself; detection keeps polling meanwhile.
 */
export const AgentInstallGuide: React.FC<AgentInstallGuideProps> = ({
  agent,
  onCheck,
  onBack,
}) => {
  const { t } = useTranslation();
  const guide = INSTALL_GUIDES[agent];
  const [copied, setCopied] = useState(false);
  const name = t(`onboarding.setup.agent.names.${agent.split("/")[1]}`);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(guide.command);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch (e) {
      console.warn("copy failed:", e);
    }
  };

  return (
    <div className="mt-2 text-left">
      <p className="text-sm text-text/70">
        {t(`onboarding.setup.agent.guide.${agent.split("/")[1]}`, { name })}
      </p>
      <div className="mt-2 flex items-center justify-between gap-2 rounded-md border border-mid-gray/20 bg-mid-gray/10 px-3 py-1.5">
        <code className="font-mono text-xs text-text break-all select-all">
          {guide.command}
        </code>
        <Button variant="ghost" size="sm" onClick={() => void copy()}>
          {copied
            ? t("onboarding.setup.agent.copied")
            : t("onboarding.setup.agent.copy")}
        </Button>
      </div>
      <div className="mt-2 flex flex-wrap items-center gap-2">
        <Button
          variant="primary"
          className="bg-logo-primary border-logo-primary hover:bg-logo-primary/90"
          onClick={onCheck}
        >
          {t("onboarding.setup.agent.check")}
        </Button>
        <Button
          variant="ghost"
          className="text-logo-primary"
          onClick={() => void openUrl(guide.docsUrl).catch(console.warn)}
        >
          {t("onboarding.setup.agent.docs")}
        </Button>
        <Button variant="ghost" onClick={onBack}>
          {t("onboarding.setup.agent.back")}
        </Button>
      </div>
      <p className="mt-2 flex items-center gap-1.5 text-sm text-text/55">
        <Loader2 className="w-3.5 h-3.5 animate-spin" aria-hidden="true" />
        {t("onboarding.setup.agent.polling")}
      </p>
    </div>
  );
};
