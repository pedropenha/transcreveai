import React from "react";
import { useTranslation } from "react-i18next";
import { AlertTriangle, MessageSquareText } from "lucide-react";
import { Button } from "../../ui/Button";
import type { SetupAgentId } from "../setupFlow";
import { AgentInstallGuide } from "./AgentInstallGuide";
import { SetupRow, StatusLine } from "./SetupRow";
import type { SetupAgentState } from "./useSetupAgent";

const shortName = (id: SetupAgentId) => id.split("/")[1];

/** Meeting-summary step: detected agent, a pick when both exist, or the
 * install / "later" choice when none is installed. */
export const AgentSetupRow: React.FC<{ state: SetupAgentState }> = ({
  state,
}) => {
  const { t } = useTranslation();
  const { statuses, detected, chosen, pick, deferred, defer, guide } = state;
  const title = t("onboarding.setup.agent.title");
  const icon = <MessageSquareText className="w-5 h-5" />;
  const nameOf = (id: SetupAgentId) =>
    t(`onboarding.setup.agent.names.${shortName(id)}`);

  if (statuses === null) {
    return <SetupRow tone="busy" icon={icon} title={title} />;
  }

  if (chosen) {
    return (
      <SetupRow
        tone="done"
        icon={icon}
        title={title}
        description={
          detected.length > 1
            ? t("onboarding.setup.agent.bothFound")
            : t("onboarding.setup.agent.using", { name: nameOf(chosen) })
        }
      >
        {detected.length > 1 ? (
          <div role="radiogroup" aria-label={title} className="mt-2 flex gap-2">
            {detected.map((id) => (
              <label
                key={id}
                className={`flex-1 flex items-center gap-2 rounded-lg border px-3 py-2 text-sm cursor-pointer ${
                  id === chosen
                    ? "border-logo-primary bg-logo-primary/10"
                    : "border-mid-gray/40"
                }`}
              >
                <input
                  type="radio"
                  name="setup-agent"
                  checked={id === chosen}
                  onChange={() => pick(id)}
                  className="accent-logo-primary"
                />
                {nameOf(id)}
              </label>
            ))}
          </div>
        ) : (
          <StatusLine tone="ok">
            {t("onboarding.setup.agent.found", { name: nameOf(chosen) })}
          </StatusLine>
        )}
      </SetupRow>
    );
  }

  if (deferred) {
    return (
      <SetupRow
        tone="pending"
        icon={icon}
        title={title}
        description={t("onboarding.setup.agent.deferred")}
      >
        <Button
          variant="ghost"
          className="mt-1 text-logo-primary"
          onClick={() => state.openGuide("cli_agent/claude")}
        >
          {t("onboarding.setup.agent.installNow")}
        </Button>
        {guide ? (
          <AgentInstallGuide
            agent={guide}
            onCheck={() => void state.refresh()}
            onBack={() => state.openGuide(null)}
          />
        ) : null}
      </SetupRow>
    );
  }

  return (
    <SetupRow
      tone="warn"
      icon={<AlertTriangle className="w-5 h-5" />}
      title={
        guide
          ? t("onboarding.setup.agent.installTitle", { name: nameOf(guide) })
          : title
      }
      description={guide ? undefined : t("onboarding.setup.agent.missing")}
    >
      {guide ? (
        <AgentInstallGuide
          agent={guide}
          onCheck={() => void state.refresh()}
          onBack={() => state.openGuide(null)}
        />
      ) : (
        <div className="mt-2 flex flex-wrap gap-2">
          {(["cli_agent/claude", "cli_agent/codex"] as const).map((id) => (
            <Button
              key={id}
              variant="secondary"
              onClick={() => state.openGuide(id)}
            >
              {t("onboarding.setup.agent.install", { name: nameOf(id) })}
            </Button>
          ))}
          <Button variant="ghost" onClick={defer}>
            {t("onboarding.setup.agent.later")}
          </Button>
        </div>
      )}
    </SetupRow>
  );
};
