/// <reference types="bun" />
import { expect, test } from "bun:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { CliAgentFields } from "./CliAgentFields";
import type { CliAgentStatus } from "@/bindings";

const status: CliAgentStatus = {
  provider_id: "cli_agent/cursor_agent",
  label: "Cursor",
  binary: "cursor-agent",
  detected: true,
  enabled: false,
  experimental: true,
  install_hint: "install cursor",
};

function checkbox(updating: boolean, enabled: boolean): string {
  const html = renderToStaticMarkup(
    <CliAgentFields
      status={status}
      config={{
        enabled,
        binary_path: null,
        extra_args: [],
        timeout_secs: null,
      }}
      updating={updating}
      onConfigChange={() => {}}
      model=""
      onModelChange={() => {}}
      modelUpdating={false}
    />,
  );
  return html.match(/<input[^>]+type="checkbox"[^>]*>/)?.[0] ?? "";
}

test("experimental configuration can be enabled and disabled", () => {
  expect(checkbox(false, false)).not.toContain("disabled");
  expect(checkbox(false, true)).not.toContain("disabled");
});
test("configuration toggle still waits for persistence", () => {
  expect(checkbox(true, false)).toContain("disabled");
});
