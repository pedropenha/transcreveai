import React from "react";
import { useTranslation } from "react-i18next";
import { useSettingsStore } from "@/stores/settingsStore";
import TranscreveTextLogo from "../../icons/TranscreveTextLogo";
import { Button } from "../../ui/Button";
import { AgentSetupRow } from "./AgentSetupRow";
import { MicTestRow } from "./MicTestRow";
import { ModelSetupRow } from "./ModelSetupRow";
import { ShortcutTestRow } from "./ShortcutTestRow";
import { useSetupAgent } from "./useSetupAgent";
import { useSetupModel } from "./useSetupModel";

interface SetupOnboardingProps {
  onComplete: () => void;
}

/**
 * Windows first run (F010): replaces the model list with a "we're getting
 * everything ready" checklist. All steps run in parallel — mic test,
 * Parakeet V3 download, shortcut check and agent detection. "Começar" waits
 * for the model and for the summary step to be settled (agent found or
 * postponed); the mic and shortcut checks are guidance, not gates.
 */
const SetupOnboarding: React.FC<SetupOnboardingProps> = ({ onComplete }) => {
  const { t } = useTranslation();
  const selectedMicrophone = useSettingsStore(
    (s) => s.settings?.selected_microphone ?? null,
  );
  const binding = useSettingsStore(
    (s) => s.settings?.bindings?.transcribe?.current_binding,
  );
  const model = useSetupModel();
  const agent = useSetupAgent();

  const agentSettled = agent.chosen !== null || agent.deferred;
  const canStart = model.phase === "ready" && agentSettled;

  return (
    <div className="h-screen w-full flex flex-col p-6 gap-4 overflow-y-auto">
      <div className="flex flex-col items-center gap-2 shrink-0">
        <TranscreveTextLogo width={200} />
        <p className="text-text/70 max-w-md font-medium mx-auto text-center">
          {t("onboarding.setup.subtitle")}
        </p>
      </div>

      <div className="max-w-[520px] w-full mx-auto flex flex-col gap-3">
        <MicTestRow selectedMicrophone={selectedMicrophone} />
        <ModelSetupRow state={model} />
        <ShortcutTestRow binding={binding} />
        <AgentSetupRow state={agent} />
      </div>

      <div className="max-w-[520px] w-full mx-auto flex justify-end pb-2">
        <Button
          variant="primary"
          size="lg"
          className="bg-logo-primary border-logo-primary hover:bg-logo-primary/90"
          disabled={!canStart}
          onClick={onComplete}
        >
          {t("onboarding.setup.start")}
        </Button>
      </div>
    </div>
  );
};

export default SetupOnboarding;
