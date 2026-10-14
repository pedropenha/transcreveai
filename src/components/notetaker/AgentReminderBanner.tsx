import React, { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { AlertTriangle, X } from "lucide-react";
import { commands, type CliAgentStatus } from "@/bindings";
import { useSettingsStore } from "@/stores/settingsStore";
import { AgentInstallGuide } from "../onboarding/setup/AgentInstallGuide";
import {
  AGENT_BANNER_DISMISSED_KEY,
  AGENT_DEFERRED_KEY,
  showAgentBanner,
  type SetupAgentId,
} from "../onboarding/setupFlow";
import { Button } from "../ui/Button";

const readFlag = (key: string): boolean => {
  try {
    return localStorage.getItem(key) === "1";
  } catch {
    return false;
  }
};

/**
 * Reminder for users who chose "Deixar para depois" on the first-run setup:
 * summaries stay off until Claude Code or Codex is installed. Disappears on
 * its own once an agent (or a working provider) is detected, or when closed.
 */
export const AgentReminderBanner: React.FC = () => {
  const { t } = useTranslation();
  const providerId = useSettingsStore(
    (s) => s.settings?.post_process_provider_id,
  );
  const apiKeyHints = useSettingsStore((s) => s.apiKeyHints);
  const [statuses, setStatuses] = useState<CliAgentStatus[] | null>(null);
  const [dismissed, setDismissed] = useState(() =>
    readFlag(AGENT_BANNER_DISMISSED_KEY),
  );
  const [guide, setGuide] = useState<SetupAgentId | null>(null);
  const deferred = readFlag(AGENT_DEFERRED_KEY);

  const refresh = useCallback(async () => {
    try {
      const result = await commands.cliAgentsStatus();
      if (result.status === "ok") setStatuses(result.data);
    } catch (e) {
      console.warn("cli_agents_status failed:", e);
    }
  }, []);

  useEffect(() => {
    if (deferred && !dismissed) void refresh();
  }, [deferred, dismissed, refresh]);

  if (
    !showAgentBanner({ deferred, dismissed, providerId, apiKeyHints, statuses })
  ) {
    return null;
  }

  const close = () => {
    try {
      localStorage.setItem(AGENT_BANNER_DISMISSED_KEY, "1");
    } catch {
      // Closing still hides it for this session.
    }
    setDismissed(true);
  };

  return (
    <div
      role="status"
      className="mb-4 rounded-lg border border-warning/25 bg-warning-soft px-3.5 py-3 text-sm"
    >
      <div className="flex items-center gap-3">
        <AlertTriangle
          className="w-5 h-5 shrink-0 text-warning"
          aria-hidden="true"
        />
        <p className="flex-1 text-text">
          <strong className="text-warning">
            {t("notetaker.agentBanner.title")}
          </strong>{" "}
          {t("notetaker.agentBanner.body")}
        </p>
        {guide === null
          ? (["cli_agent/claude", "cli_agent/codex"] as const).map((id) => (
              <Button key={id} variant="warning" onClick={() => setGuide(id)}>
                {t("onboarding.setup.agent.install", {
                  name: t(`onboarding.setup.agent.names.${id.split("/")[1]}`),
                })}
              </Button>
            ))
          : null}
        <button
          type="button"
          className="icon-button"
          aria-label={t("notetaker.agentBanner.close")}
          title={t("notetaker.agentBanner.close")}
          onClick={close}
        >
          <X width={16} height={16} aria-hidden="true" />
        </button>
      </div>
      {guide ? (
        <AgentInstallGuide
          agent={guide}
          onCheck={() => void refresh()}
          onBack={() => setGuide(null)}
        />
      ) : null}
    </div>
  );
};
