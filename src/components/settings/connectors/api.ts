import { invoke } from "@tauri-apps/api/core";

// Keep the temporary IPC boundary here until tauri-specta regenerates bindings.
// These shapes mirror src-tauri/src/connectors/types.rs and contain no secrets.
export type ConnectorKind = "notion" | "azure_devops";
export type ConnectorStatus =
  | "disconnected"
  | "authorizing"
  | "ready"
  | "reconnect_required"
  | "temporary_failure";
export type SprintPolicy = "ask" | "fixed" | "current_team";
export interface ConnectorScope {
  notion_page_ids: string[];
  azure_project_ids: string[];
}
export interface AzureDestinationDefaults {
  project_id: string;
  team_id: string;
  backlog_id: string | null;
  area_path: string | null;
  work_item_type: string | null;
  sprint_policy: SprintPolicy;
  iteration_id: string | null;
  iteration_path: string | null;
}
export interface ConnectorInput {
  kind: ConnectorKind;
  label: string;
  enabled: boolean;
  organization: string | null;
  tenant: string | null;
  client_id: string | null;
  scope: ConnectorScope;
  default_notion_page_id: string | null;
}
export interface ConnectorConfig extends ConnectorInput {
  id: string;
  policy_revision: number;
  status: ConnectorStatus;
  identity_hint: string | null;
  azure_defaults: AzureDestinationDefaults | null;
  oauth_redirect: string | null;
}
export interface CatalogEntry {
  id: string;
  name: string;
  path: string | null;
  start_date: string | null;
  finish_date: string | null;
  is_current: boolean;
}
export interface AzureCatalog {
  projects: CatalogEntry[];
  teams: CatalogEntry[];
  backlogs: CatalogEntry[];
  areas: CatalogEntry[];
  iterations: CatalogEntry[];
  work_item_types: CatalogEntry[];
}
export interface ConnectionTest {
  identity_hint: string | null;
  readable: boolean;
  publication: string;
}
export interface ConnectorError {
  code: string;
  message: string;
}

export const connectorApi = {
  list: () => invoke<ConnectorConfig[]>("connector_list_connections"),
  save: (input: ConnectorInput, id?: string) =>
    invoke<ConnectorConfig>("connector_save_connection", {
      input,
      id: id ?? null,
    }),
  beginOAuth: (connectionId: string) =>
    invoke<ConnectorConfig>("connector_begin_oauth", { connectionId }),
  cancelOAuth: (connectionId: string) =>
    invoke<void>("connector_cancel_oauth", { connectionId }),
  disconnect: (connectionId: string) =>
    invoke<void>("connector_disconnect", { connectionId }),
  test: (connectionId: string) =>
    invoke<ConnectionTest>("connector_test_connection", { connectionId }),
  catalog: (connectionId: string, projectId?: string, teamId?: string) =>
    invoke<AzureCatalog>("connector_azure_catalog", {
      connectionId,
      projectId: projectId ?? null,
      teamId: teamId ?? null,
    }),
  saveDefaults: (connectionId: string, defaults: AzureDestinationDefaults) =>
    invoke<ConnectorConfig>("connector_save_defaults", {
      connectionId,
      defaults,
    }),
};
