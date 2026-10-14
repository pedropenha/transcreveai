import { useCallback, useEffect, useRef, useState } from "react";
import { commands, type CliAgentStatus } from "@/bindings";
import { useSettingsStore } from "@/stores/settingsStore";
import {
  AGENT_DEFERRED_KEY,
  agentToAdopt,
  detectedAgents,
  type SetupAgentId,
} from "../setupFlow";

/** Detection is a PATH/registry file check — cheap enough to poll. */
const POLL_MS = 3000;

const storeFlag = (key: string, on: boolean) => {
  try {
    if (on) localStorage.setItem(key, "1");
    else localStorage.removeItem(key);
  } catch {
    // localStorage unavailable: the reminder simply will not show.
  }
};

export interface SetupAgentState {
  statuses: CliAgentStatus[] | null;
  detected: SetupAgentId[];
  /** Agent summaries will use (the explicit pick or the preferred one). */
  chosen: SetupAgentId | null;
  pick: (id: SetupAgentId) => void;
  deferred: boolean;
  defer: () => void;
  /** Install instructions currently open, if any. */
  guide: SetupAgentId | null;
  openGuide: (id: SetupAgentId | null) => void;
  refresh: () => Promise<void>;
}

/**
 * Meeting-summary step: finds Claude Code / Codex and makes it the summary
 * provider when the current one cannot run (fresh install: `openai` without
 * a key). Keeps polling while nothing is installed so an install done in
 * another window shows up without a restart.
 */
export const useSetupAgent = (): SetupAgentState => {
  const providerId = useSettingsStore(
    (s) => s.settings?.post_process_provider_id,
  );
  const apiKeyHints = useSettingsStore((s) => s.apiKeyHints);
  const setPostProcessProvider = useSettingsStore(
    (s) => s.setPostProcessProvider,
  );
  const [statuses, setStatuses] = useState<CliAgentStatus[] | null>(null);
  const [picked, setPicked] = useState<SetupAgentId | null>(null);
  const [deferred, setDeferred] = useState(false);
  const [guide, setGuide] = useState<SetupAgentId | null>(null);
  const adopting = useRef(false);

  const refresh = useCallback(async () => {
    try {
      const result = await commands.cliAgentsStatus();
      if (result.status === "ok") setStatuses(result.data);
    } catch (e) {
      console.warn("cli_agents_status failed:", e);
    }
  }, []);

  const detected = statuses ? detectedAgents(statuses) : [];
  const found = detected.length > 0;

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    if (found) return;
    const timer = setInterval(() => void refresh(), POLL_MS);
    return () => clearInterval(timer);
  }, [found, refresh]);

  useEffect(() => {
    if (found) {
      setGuide(null);
      setDeferred(false);
      storeFlag(AGENT_DEFERRED_KEY, false);
    }
  }, [found]);

  useEffect(() => {
    if (!statuses || adopting.current) return;
    const target = agentToAdopt(picked, providerId, apiKeyHints, statuses);
    if (!target) return;
    adopting.current = true;
    void setPostProcessProvider(target).finally(() => {
      adopting.current = false;
    });
  }, [statuses, picked, providerId, apiKeyHints, setPostProcessProvider]);

  // Reflect what summaries really use: an explicit pick, else an agent that
  // is already the provider, else the preferred detected one.
  const current = detected.find((id) => id === providerId);
  const chosen =
    picked && detected.includes(picked)
      ? picked
      : (current ?? detected[0] ?? null);

  return {
    statuses,
    detected,
    chosen,
    pick: setPicked,
    deferred,
    defer: () => {
      setDeferred(true);
      setGuide(null);
      storeFlag(AGENT_DEFERRED_KEY, true);
    },
    guide,
    openGuide: setGuide,
    refresh,
  };
};
