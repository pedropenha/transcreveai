import type {
  AzureCatalog,
  AzureDestinationDefaults,
  CatalogEntry,
} from "./api";

const NOTION_PAGE_ID =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

export function normalizeNotionPages(value: string): string[] {
  const ids = value
    .split(/[\s,]+/)
    .map((id) => id.toLowerCase())
    .filter(Boolean);
  if (ids.some((id) => !NOTION_PAGE_ID.test(id)))
    throw new Error("invalid_notion_page_id");
  return [...new Set(ids)];
}

export function selectCatalogValue(
  entries: CatalogEntry[],
  id: string | null,
): string | null {
  return id && entries.some((entry) => entry.id === id) ? id : null;
}

export function buildAzureDefaults(
  proposed: AzureDestinationDefaults,
  catalog: AzureCatalog,
): AzureDestinationDefaults | null {
  const projectId = selectCatalogValue(catalog.projects, proposed.project_id);
  const teamId = selectCatalogValue(catalog.teams, proposed.team_id);
  if (!projectId || !teamId) return null;
  const iteration =
    proposed.sprint_policy === "fixed"
      ? catalog.iterations.find((item) => item.id === proposed.iteration_id)
      : undefined;
  if (proposed.sprint_policy === "fixed" && !iteration) return null;
  const area = catalog.areas.find((item) => item.path === proposed.area_path);
  return {
    project_id: projectId,
    team_id: teamId,
    backlog_id: selectCatalogValue(catalog.backlogs, proposed.backlog_id),
    area_path: area?.path ?? null,
    work_item_type: selectCatalogValue(
      catalog.work_item_types,
      proposed.work_item_type,
    ),
    sprint_policy: proposed.sprint_policy,
    iteration_id: iteration?.id ?? null,
    iteration_path: iteration?.path ?? null,
  };
}
