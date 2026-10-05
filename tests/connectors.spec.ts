import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";
import { installTauriMock, type CommandHandlers } from "./helpers/tauri-mock";

const id = "11111111-1111-4111-8111-111111111111";
const projectId = "22222222-2222-4222-8222-222222222222";
const config = {
  id,
  kind: "azure_devops",
  label: "Team planning",
  enabled: true,
  organization: "example-org",
  tenant: null,
  client_id: null,
  scope: { notion_page_ids: [], azure_project_ids: [projectId] },
  default_notion_page_id: null,
  policy_revision: 1,
  status: "disconnected",
  identity_hint: null,
  azure_defaults: null,
  oauth_redirect: null,
};

async function open(page: Page, extra: CommandHandlers = {}) {
  const mock = await installTauriMock(page, {
    get_app_settings: { onboarding_completed: true, app_language: "en" },
    connector_list_connections: [config],
    mcp_status: { running: false, grants: [] },
    ...extra,
  });
  await page.goto("/");
  await page.getByTitle("Settings", { exact: true }).click();
  await page
    .getByRole("navigation", { name: "Settings sections" })
    .getByRole("button", { name: "Connectors", exact: true })
    .click();
  return mock;
}

test("built-in Entra app starts OAuth without a user registration", async ({
  page,
}) => {
  let authorizing = false;
  const mock = await open(page, {
    connector_begin_oauth: () => {
      authorizing = true;
      return { ...config, status: "authorizing" };
    },
    connector_list_connections: () => [
      { ...config, status: authorizing ? "authorizing" : "disconnected" },
    ],
  });
  await expect(page.getByText("Team planning", { exact: true })).toBeVisible();
  await expect(page.getByTestId("connectors-page")).toBeVisible();
  await expect(
    page.getByText(/built-in Transcreve\.ai Entra application/i).first(),
  ).toBeVisible();
  const connect = page.getByRole("button", {
    name: "Connect in browser",
    exact: true,
  });
  await expect(connect).toBeEnabled();
  expect(mock.calls.some((call) => call.cmd === "connector_begin_oauth")).toBe(
    false,
  );
  await connect.click();
  expect(mock.calls.some((call) => call.cmd === "connector_begin_oauth")).toBe(
    true,
  );
  await expect(
    page.getByRole("button", { name: /cancel/i }).first(),
  ).toBeVisible();
});

test("disconnected service does not discover catalogs or publish in background", async ({
  page,
}) => {
  const mock = await open(page);
  await expect(page.getByText("Team planning", { exact: true })).toBeVisible();
  expect(
    mock.calls.some((call) => call.cmd === "connector_azure_catalog"),
  ).toBe(false);
  expect(
    mock.calls.some((call) =>
      /publish|execute_action|prepare_action/.test(call.cmd),
    ),
  ).toBe(false);
});

test("connector settings are keyboard-accessible without serious axe violations", async ({
  page,
}) => {
  await open(page);
  await expect(page.getByTestId("connectors-page")).toBeVisible();
  const results = await new AxeBuilder({ page })
    .include('[data-testid="connectors-page"]')
    .disableRules(["color-contrast"])
    .analyze();
  expect(
    results.violations.filter(
      (v) => v.impact === "critical" || v.impact === "serious",
    ),
  ).toEqual([]);
});
