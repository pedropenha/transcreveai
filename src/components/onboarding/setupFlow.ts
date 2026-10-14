import type { CliAgentStatus, ModelInfo } from "@/bindings";

/**
 * Pure rules behind the Windows first-run setup screen (F010): which model is
 * installed, which agent CLI writes meeting summaries, and how the shortcut
 * check matches keys. Kept React-free so they can be unit-tested with `bun`.
 */

/** Multilingual Parakeet (25 languages incl. Portuguese) — the only model the
 * setup downloads. English-only models are never fetched. */
export const SETUP_MODEL_SLUG = "parakeet-tdt-0.6b-v3";

/** Order used when several agents are installed. */
export const AGENT_PREFERENCE = ["cli_agent/claude", "cli_agent/codex"];

export type SetupAgentId = "cli_agent/claude" | "cli_agent/codex";

export interface InstallGuide {
  command: string;
  docsUrl: string;
}

/** Official Windows install instructions shown when no agent is found. */
export const INSTALL_GUIDES: Record<SetupAgentId, InstallGuide> = {
  "cli_agent/claude": {
    command: "irm https://claude.ai/install.ps1 | iex",
    docsUrl: "https://code.claude.com/docs/en/setup",
  },
  "cli_agent/codex": {
    command: "npm install -g @openai/codex",
    docsUrl: "https://developers.openai.com/codex/cli",
  },
};

const isLegacy = (model: ModelInfo): boolean =>
  typeof model.source === "object" && "Url" in model.source;

/**
 * The catalog (GGUF) Parakeet V3 entry, preferring one already on disk. The
 * legacy ONNX blob with the same slug is never chosen.
 */
export const pickSetupModel = (models: ModelInfo[]): ModelInfo | undefined => {
  const candidates = models.filter(
    (m) => !isLegacy(m) && m.id.includes(SETUP_MODEL_SLUG),
  );
  return candidates.find((m) => m.is_downloaded) ?? candidates[0];
};

/** Installed, enabled, non-experimental agents in preference order. */
export const detectedAgents = (statuses: CliAgentStatus[]): SetupAgentId[] =>
  AGENT_PREFERENCE.filter((id) =>
    statuses.some(
      (s) => s.provider_id === id && s.detected && s.enabled && !s.experimental,
    ),
  ) as SetupAgentId[];

/** Mirrors `llm::router::requires_api_key`. */
export const requiresApiKey = (providerId: string): boolean =>
  providerId !== "custom" &&
  providerId !== "apple_intelligence" &&
  !providerId.startsWith("cli_agent/");

/**
 * Whether the active summary provider can actually run: a detected CLI agent,
 * a BYOK provider with a saved key, or a keyless provider (custom/local).
 */
export const providerReady = (
  providerId: string | undefined,
  apiKeyHints: Record<string, string>,
  statuses: CliAgentStatus[],
): boolean => {
  if (!providerId) return false;
  if (providerId.startsWith("cli_agent/")) {
    return statuses.some(
      (s) => s.provider_id === providerId && s.detected && s.enabled,
    );
  }
  return !requiresApiKey(providerId) || providerId in apiKeyHints;
};

/**
 * The agent the setup should switch summaries to, or `null` to leave the
 * provider as is. `picked` is an explicit choice made on the setup screen;
 * without one, a provider that already works is never overwritten.
 */
export const agentToAdopt = (
  picked: SetupAgentId | null,
  providerId: string | undefined,
  apiKeyHints: Record<string, string>,
  statuses: CliAgentStatus[],
): SetupAgentId | null => {
  const agents = detectedAgents(statuses);
  if (agents.length === 0) return null;
  const choice = picked && agents.includes(picked) ? picked : agents[0];
  if (providerId === choice) return null;
  if (picked === null && providerReady(providerId, apiKeyHints, statuses)) {
    return null;
  }
  return choice;
};

/** "ctrl+space" → ["ctrl", "space"]. */
export const shortcutKeys = (binding: string | undefined): string[] =>
  (binding ?? "")
    .split("+")
    .map((k) => k.trim().toLowerCase())
    .filter(Boolean);

/** Binding keys that are part of the current press (for the per-key chips). */
export const hitKeys = (
  keys: string[],
  pressed: ReadonlySet<string>,
): Set<string> => new Set(keys.filter((k) => pressed.has(k)));

/** The whole combination is held at once. */
export const shortcutMatched = (
  keys: string[],
  pressed: ReadonlySet<string>,
): boolean => keys.length > 0 && keys.every((k) => pressed.has(k));

/** localStorage flag set by "Deixar para depois". */
export const AGENT_DEFERRED_KEY = "transcreve.setup.agentDeferred";
/** localStorage flag set when the Notetaker banner is closed. */
export const AGENT_BANNER_DISMISSED_KEY =
  "transcreve.notetaker.agentBannerDismissed";

/**
 * Notetaker reminder: the user postponed installing an agent, never closed the
 * reminder, and summaries still have no working provider.
 */
export const showAgentBanner = (input: {
  deferred: boolean;
  dismissed: boolean;
  providerId: string | undefined;
  apiKeyHints: Record<string, string>;
  statuses: CliAgentStatus[] | null;
}): boolean =>
  input.deferred &&
  !input.dismissed &&
  input.statuses !== null &&
  detectedAgents(input.statuses).length === 0 &&
  !providerReady(input.providerId, input.apiKeyHints, input.statuses);
