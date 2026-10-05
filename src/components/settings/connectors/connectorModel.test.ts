import { expect, test } from "bun:test";
import {
  buildAzureDefaults,
  normalizeNotionPages,
  selectCatalogValue,
} from "./connectorModel";
import type { AzureCatalog, AzureDestinationDefaults } from "./api";

const entry = (id: string) => ({
  id,
  name: id,
  path: id,
  start_date: null,
  finish_date: null,
  is_current: false,
});
const catalog: AzureCatalog = {
  projects: [entry("project-a")],
  teams: [entry("team-a")],
  backlogs: [entry("backlog-a")],
  areas: [entry("area-a")],
  iterations: [entry("sprint-a")],
  work_item_types: [entry("type-a")],
};
const defaults: AzureDestinationDefaults = {
  project_id: "project-a",
  team_id: "team-a",
  backlog_id: "backlog-a",
  area_path: "area-a",
  work_item_type: "type-a",
  sprint_policy: "fixed",
  iteration_id: "sprint-a",
  iteration_path: "sprint-a",
};

test("Notion roots accept canonical UUIDs once and reject arbitrary text", () => {
  const id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
  expect(normalizeNotionPages(`${id}\n${id.toUpperCase()}`)).toEqual([id]);
  expect(() => normalizeNotionPages("workspace name")).toThrow();
});

test("catalog selection only uses IDs returned by the service", () => {
  expect(selectCatalogValue(catalog.projects, "project-a")).toBe("project-a");
  expect(selectCatalogValue(catalog.projects, "invented")).toBeNull();
});

test("changing catalog context invalidates dependent defaults", () => {
  expect(buildAzureDefaults(defaults, { ...catalog, teams: [] })).toBeNull();
  expect(
    buildAzureDefaults(defaults, { ...catalog, iterations: [] }),
  ).toBeNull();
  expect(
    buildAzureDefaults(defaults, { ...catalog, work_item_types: [] })
      ?.work_item_type,
  ).toBeNull();
  expect(
    buildAzureDefaults({ ...defaults, sprint_policy: "ask" }, catalog)
      ?.iteration_id,
  ).toBeNull();
  expect(buildAzureDefaults(defaults, catalog)).toEqual(defaults);
});
