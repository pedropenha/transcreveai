/// <reference types="bun" />
import { describe, expect, test } from "bun:test";
import { assistantProviderOption } from "./assistantProviderOptions";

describe("assistant CLI provider availability", () => {
  const option = {
    value: "cli_agent/cursor_agent",
    label: "Cursor — experimental",
    disabled: true,
  };
  test("missing experimental stays disabled", () => {
    expect(
      assistantProviderOption(option, { detected: false, enabled: true })
        .disabled,
    ).toBe(true);
  });
  test("disabled experimental stays disabled", () => {
    expect(
      assistantProviderOption(option, { detected: true, enabled: false })
        .disabled,
    ).toBe(true);
  });
  test("explicit selection can use installed enabled experimental", () => {
    expect(
      assistantProviderOption(option, { detected: true, enabled: true })
        .disabled,
    ).toBe(false);
  });
  test("unknown detection waits before allowing CLI selection", () => {
    expect(assistantProviderOption(option, undefined).disabled).toBe(true);
  });
  test("installed disabled stable agent stays disabled", () => {
    expect(
      assistantProviderOption(
        { value: "cli_agent/codex", label: "Codex" },
        { detected: true, enabled: false },
      ).disabled,
    ).toBe(true);
  });
  test("BYOK availability is preserved", () => {
    const byok = { value: "openai", label: "OpenAI" };
    expect(assistantProviderOption(byok, undefined)).toEqual(byok);
  });
});
