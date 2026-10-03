import type { CliAgentStatus } from "@/bindings";
import type { DropdownOption } from "../ui/Dropdown";

/** Explicit assistant selection still requires an installed, enabled CLI. */
export function assistantProviderOption(
  option: DropdownOption,
  status: Pick<CliAgentStatus, "detected" | "enabled"> | undefined,
): DropdownOption {
  if (!option.value.startsWith("cli_agent/")) return option;
  return { ...option, disabled: !status?.detected || !status.enabled };
}
