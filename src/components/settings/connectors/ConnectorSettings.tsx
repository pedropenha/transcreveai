import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  connectorApi,
  type AzureCatalog,
  type AzureDestinationDefaults,
  type CatalogEntry,
  type ConnectorConfig,
  type ConnectorInput,
  type ConnectorKind,
  type ConnectionTest,
  type SprintPolicy,
} from "./api";
import { buildAzureDefaults, normalizeNotionPages } from "./connectorModel";
import { McpClientSettings } from "./McpClientSettings";
import "./connectors.css";

const blank = (kind: ConnectorKind): ConnectorInput => ({
  kind,
  label: "",
  enabled: true,
  organization: null,
  tenant: null,
  client_id: null,
  scope: { notion_page_ids: [], azure_project_ids: [] },
  default_notion_page_id: null,
});

const asMessage = (error: unknown): string => {
  if (typeof error === "object" && error !== null && "code" in error) {
    const code = (error as { code: unknown }).code;
    if (typeof code === "string") return code;
  }
  return "temporary_failure";
};

const findPath = (items: CatalogEntry[], id: string | null) =>
  items.find((item) => item.id === id)?.path ?? null;

interface ChoiceProps {
  label: string;
  value: string | null;
  options: CatalogEntry[];
  onChange: (value: string | null) => void;
  required?: boolean;
  disabled?: boolean;
}

function CatalogChoice({
  label,
  value,
  options,
  onChange,
  required,
  disabled,
}: ChoiceProps) {
  const { t } = useTranslation();
  return (
    <label className="connector-field">
      <span>{label}</span>
      <select
        value={value ?? ""}
        onChange={(event) => onChange(event.target.value || null)}
        required={required}
        disabled={disabled}
      >
        <option value="">{t("connectors.choose")}</option>
        {options.map((option) => (
          <option key={option.id} value={option.id}>
            {option.name}
          </option>
        ))}
      </select>
    </label>
  );
}

function AzureDefaults({
  connection,
  onSaved,
}: {
  connection: ConnectorConfig;
  onSaved: (config: ConnectorConfig) => void;
}) {
  const { t } = useTranslation();
  const [catalog, setCatalog] = useState<AzureCatalog | null>(null);
  const [draft, setDraft] = useState<AzureDestinationDefaults>(
    () =>
      connection.azure_defaults ?? {
        project_id: "",
        team_id: "",
        backlog_id: null,
        area_path: null,
        work_item_type: null,
        sprint_policy: "ask",
        iteration_id: null,
        iteration_path: null,
      },
  );
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    let active = true;
    connectorApi
      .catalog(
        connection.id,
        draft.project_id || undefined,
        draft.team_id || undefined,
      )
      .then((value) => {
        if (active) setCatalog(value);
      })
      .catch((reason: unknown) => {
        if (active) setError(asMessage(reason));
      });
    return () => {
      active = false;
    };
  }, [connection.id, draft.project_id, draft.team_id]);

  const update = (patch: Partial<AzureDestinationDefaults>) =>
    setDraft((old) => ({ ...old, ...patch }));
  const save = async () => {
    if (!catalog) return;
    const checked = buildAzureDefaults(draft, catalog);
    if (!checked) {
      setError("invalid_destination");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      onSaved(await connectorApi.saveDefaults(connection.id, checked));
    } catch (reason) {
      setError(asMessage(reason));
    } finally {
      setBusy(false);
    }
  };
  return (
    <section
      className="connector-defaults"
      aria-label={t("connectors.azureDefaults")}
    >
      <h3>{t("connectors.azureDefaults")}</h3>
      {!catalog ? (
        <p>{t("connectors.loadingCatalog")}</p>
      ) : (
        <div className="connector-grid">
          <CatalogChoice
            label={t("connectors.project")}
            value={draft.project_id}
            options={catalog.projects}
            required
            onChange={(id) =>
              update({
                project_id: id ?? "",
                team_id: "",
                backlog_id: null,
                area_path: null,
                work_item_type: null,
                iteration_id: null,
                iteration_path: null,
              })
            }
          />
          <CatalogChoice
            label={t("connectors.team")}
            value={draft.team_id}
            options={catalog.teams}
            required
            disabled={!draft.project_id}
            onChange={(id) =>
              update({
                team_id: id ?? "",
                backlog_id: null,
                area_path: null,
                work_item_type: null,
                iteration_id: null,
                iteration_path: null,
              })
            }
          />
          <CatalogChoice
            label={t("connectors.backlog")}
            value={draft.backlog_id}
            options={catalog.backlogs}
            disabled={!draft.team_id}
            onChange={(id) => update({ backlog_id: id })}
          />
          <CatalogChoice
            label={t("connectors.area")}
            value={
              catalog.areas.find((item) => item.path === draft.area_path)?.id ??
              null
            }
            options={catalog.areas}
            disabled={!draft.team_id}
            onChange={(id) =>
              update({ area_path: findPath(catalog.areas, id) })
            }
          />
          <CatalogChoice
            label={t("connectors.workItemType")}
            value={draft.work_item_type}
            options={catalog.work_item_types}
            disabled={!draft.team_id}
            onChange={(id) => update({ work_item_type: id })}
          />
          <label className="connector-field">
            <span>{t("connectors.sprintPolicy")}</span>
            <select
              value={draft.sprint_policy}
              disabled={!draft.team_id}
              onChange={(event) =>
                update({
                  sprint_policy: event.target.value as SprintPolicy,
                  iteration_id: null,
                  iteration_path: null,
                })
              }
            >
              <option value="ask">{t("connectors.sprintAsk")}</option>
              <option value="fixed">{t("connectors.sprintFixed")}</option>
              <option value="current_team">
                {t("connectors.sprintCurrent")}
              </option>
            </select>
          </label>
          {draft.sprint_policy === "fixed" && (
            <CatalogChoice
              label={t("connectors.iteration")}
              value={draft.iteration_id}
              options={catalog.iterations}
              required
              onChange={(id) =>
                update({
                  iteration_id: id,
                  iteration_path: findPath(catalog.iterations, id),
                })
              }
            />
          )}
        </div>
      )}
      {error && (
        <p role="alert">
          {t(`connectors.errors.${error}`, {
            defaultValue: t("connectors.errors.temporary_failure"),
          })}
        </p>
      )}
      <button
        className="connector-button"
        type="button"
        onClick={save}
        disabled={busy || !catalog || !draft.project_id || !draft.team_id}
      >
        {t("connectors.saveDefaults")}
      </button>
    </section>
  );
}

export function ConnectorSettings() {
  const { t } = useTranslation();
  const [connections, setConnections] = useState<ConnectorConfig[]>([]);
  const [editing, setEditing] = useState<string | "new" | null>(null);
  const [form, setForm] = useState<ConnectorInput>(blank("notion"));
  const [notionPages, setNotionPages] = useState("");
  const [catalogs, setCatalogs] = useState<Record<string, AzureCatalog>>({});
  const [tests, setTests] = useState<Record<string, ConnectionTest>>({});
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(
    async () => setConnections(await connectorApi.list()),
    [],
  );
  useEffect(() => {
    refresh().catch((reason: unknown) => setError(asMessage(reason)));
  }, [refresh]);
  useEffect(() => {
    if (!connections.some((connection) => connection.status === "authorizing"))
      return;
    const timer = window.setInterval(() => {
      refresh().catch((reason: unknown) => setError(asMessage(reason)));
    }, 2000);
    return () => window.clearInterval(timer);
  }, [connections, refresh]);

  const edit = (connection?: ConnectorConfig) => {
    setError(null);
    setEditing(connection?.id ?? "new");
    setForm(
      connection
        ? {
            kind: connection.kind,
            label: connection.label,
            enabled: connection.enabled,
            organization: connection.organization,
            tenant: connection.tenant,
            client_id: connection.client_id,
            scope: connection.scope,
            default_notion_page_id: connection.default_notion_page_id,
          }
        : blank("notion"),
    );
    setNotionPages(connection?.scope.notion_page_ids.join("\n") ?? "");
    if (connection?.kind === "azure_devops") loadCatalog(connection.id);
  };
  const loadCatalog = (id: string) => {
    connectorApi
      .catalog(id)
      .then((catalog) => setCatalogs((old) => ({ ...old, [id]: catalog })))
      .catch((reason: unknown) => setError(asMessage(reason)));
  };
  const run = async (key: string, action: () => Promise<unknown>) => {
    setBusy(key);
    setError(null);
    try {
      await action();
      await refresh();
    } catch (reason) {
      setError(asMessage(reason));
    } finally {
      setBusy(null);
    }
  };
  const save = async () => {
    let input = form;
    if (form.kind === "notion") {
      try {
        const ids = normalizeNotionPages(notionPages);
        input = {
          ...form,
          scope: { notion_page_ids: ids, azure_project_ids: [] },
          default_notion_page_id: ids.includes(
            form.default_notion_page_id ?? "",
          )
            ? form.default_notion_page_id
            : null,
        };
      } catch {
        setError("invalid_notion_page_id");
        return;
      }
    }
    await run("save", async () => {
      await connectorApi.save(
        input,
        editing === "new" ? undefined : (editing ?? undefined),
      );
      setEditing(null);
    });
  };
  const update = (patch: Partial<ConnectorInput>) =>
    setForm((old) => ({ ...old, ...patch }));
  const kindName = (kind: ConnectorKind) =>
    kind === "notion" ? "Notion" : "Azure DevOps";
  return (
    <div className="connector-page" data-testid="connectors-page">
      <p className="connector-intro">{t("connectors.intro")}</p>
      <div className="connector-actions">
        <button
          type="button"
          className="connector-button"
          onClick={() => {
            edit();
            update(blank("notion"));
          }}
        >
          {t("connectors.addNotion")}
        </button>
        <button
          type="button"
          className="connector-button secondary"
          onClick={() => {
            edit();
            update(blank("azure_devops"));
          }}
        >
          {t("connectors.addAzure")}
        </button>
      </div>
      {error && (
        <p role="alert" className="connector-error">
          {t(`connectors.errors.${error}`, {
            defaultValue: t("connectors.errors.temporary_failure"),
          })}
        </p>
      )}
      {editing && (
        <section
          className="connector-card"
          aria-label={t("connectors.configuration")}
        >
          <h2>{t(editing === "new" ? "connectors.add" : "connectors.edit")}</h2>
          <div className="connector-grid">
            {editing === "new" && (
              <label className="connector-field">
                <span>{t("connectors.service")}</span>
                <select
                  value={form.kind}
                  onChange={(event) => {
                    update(blank(event.target.value as ConnectorKind));
                    setNotionPages("");
                  }}
                >
                  <option value="notion">{kindName("notion")}</option>
                  <option value="azure_devops">
                    {kindName("azure_devops")}
                  </option>
                </select>
              </label>
            )}
            <label className="connector-field">
              <span>{t("connectors.name")}</span>
              <input
                value={form.label}
                maxLength={120}
                required
                onChange={(event) => update({ label: event.target.value })}
              />
            </label>
            {form.kind === "azure_devops" && (
              <>
                <p className="connector-help connector-wide">
                  {t("connectors.entraGuide")}
                </p>
                <button
                  type="button"
                  className="connector-link connector-wide"
                  onClick={() =>
                    openUrl(
                      "https://learn.microsoft.com/en-us/azure/devops/integrate/get-started/authentication/authentication-guidance?view=azure-devops",
                    )
                  }
                >
                  {t("connectors.entraDocs")}
                </button>
                <label className="connector-field">
                  <span>{t("connectors.clientId")}</span>
                  <input
                    value={form.client_id ?? ""}
                    aria-describedby="azure-client-id-hint"
                    onChange={(event) =>
                      update({ client_id: event.target.value || null })
                    }
                  />
                </label>
                <p
                  id="azure-client-id-hint"
                  className="connector-help connector-wide"
                >
                  {t("connectors.clientIdHint")}
                </p>
                <label className="connector-field">
                  <span>{t("connectors.tenant")}</span>
                  <input
                    value={form.tenant ?? ""}
                    onChange={(event) =>
                      update({ tenant: event.target.value || null })
                    }
                  />
                </label>
                <label className="connector-field">
                  <span>{t("connectors.organization")}</span>
                  <input
                    value={form.organization ?? ""}
                    onChange={(event) =>
                      update({ organization: event.target.value || null })
                    }
                  />
                </label>
                {editing !== "new" && catalogs[editing] && (
                  <fieldset className="connector-scope">
                    <legend>{t("connectors.allowedProjects")}</legend>
                    {catalogs[editing].projects.map((project) => (
                      <label key={project.id}>
                        <input
                          type="checkbox"
                          checked={form.scope.azure_project_ids.includes(
                            project.id,
                          )}
                          onChange={(event) =>
                            update({
                              scope: {
                                ...form.scope,
                                azure_project_ids: event.target.checked
                                  ? [
                                      ...form.scope.azure_project_ids,
                                      project.id,
                                    ]
                                  : form.scope.azure_project_ids.filter(
                                      (id) => id !== project.id,
                                    ),
                              },
                            })
                          }
                        />
                        {project.name}
                      </label>
                    ))}
                  </fieldset>
                )}
              </>
            )}
            {form.kind === "notion" && (
              <>
                <label className="connector-field">
                  <span>{t("connectors.allowedPages")}</span>
                  <textarea
                    value={notionPages}
                    rows={3}
                    onChange={(event) => setNotionPages(event.target.value)}
                    aria-describedby="notion-pages-help"
                  />
                </label>
                <p id="notion-pages-help" className="connector-help">
                  {t("connectors.pageIdsHelp")}
                </p>
                <label className="connector-field">
                  <span>{t("connectors.defaultPage")}</span>
                  <select
                    value={form.default_notion_page_id ?? ""}
                    onChange={(event) =>
                      update({
                        default_notion_page_id: event.target.value || null,
                      })
                    }
                  >
                    <option value="">{t("connectors.none")}</option>
                    {notionPages
                      .split(/[\s,]+/)
                      .filter(Boolean)
                      .map((id) => (
                        <option key={id} value={id}>
                          {id}
                        </option>
                      ))}
                  </select>
                </label>
              </>
            )}
          </div>
          <div className="connector-actions">
            <button
              type="button"
              className="connector-button"
              disabled={busy !== null || !form.label.trim()}
              onClick={save}
            >
              {t("connectors.save")}
            </button>
            <button
              type="button"
              className="connector-button secondary"
              onClick={() => setEditing(null)}
            >
              {t("connectors.cancel")}
            </button>
          </div>
        </section>
      )}
      {connections.length === 0 && <p>{t("connectors.empty")}</p>}
      {connections.map((connection) => (
        <section
          className="connector-card"
          data-testid={`connector-card-${connection.id}`}
          key={connection.id}
          aria-label={`${kindName(connection.kind)}: ${connection.label}`}
        >
          <div className="connector-heading">
            <div>
              <h2>{connection.label}</h2>
              <p>
                {kindName(connection.kind)}
                {connection.identity_hint
                  ? ` · ${connection.identity_hint}`
                  : ""}
              </p>
            </div>
            <span
              className="st-chip st-chip-sm"
              data-tone={connection.status === "ready" ? "success" : "outline"}
            >
              {t(`connectors.status.${connection.status}`)}
            </span>
          </div>
          {connection.status === "ready" && (
            <p className="connector-help">
              {t("connectors.publicationUnknown")}
            </p>
          )}
          {connection.kind === "azure_devops" && !connection.client_id && (
            <>
              <p className="connector-help">{t("connectors.builtinEntra")}</p>
              <button
                type="button"
                className="connector-link"
                onClick={() =>
                  openUrl(
                    "https://learn.microsoft.com/en-us/azure/devops/integrate/get-started/authentication/authentication-guidance?view=azure-devops",
                  )
                }
              >
                {t("connectors.entraDocs")}
              </button>
            </>
          )}
          {connection.kind === "azure_devops" && connection.oauth_redirect && (
            <p className="connector-help">
              {t("connectors.redirect")}:{" "}
              <code>{connection.oauth_redirect}</code>
            </p>
          )}
          <div className="connector-actions">
            {connection.status === "authorizing" ? (
              <button
                type="button"
                className="connector-button secondary"
                disabled={busy !== null}
                onClick={() =>
                  run(connection.id, () =>
                    connectorApi.cancelOAuth(connection.id),
                  )
                }
              >
                {t("connectors.cancelAuthorization")}
              </button>
            ) : (
              <button
                type="button"
                className="connector-button"
                disabled={busy !== null}
                onClick={() =>
                  run(connection.id, () =>
                    connectorApi.beginOAuth(connection.id),
                  )
                }
              >
                {t(
                  connection.status === "ready"
                    ? "connectors.reconnect"
                    : "connectors.connect",
                )}
              </button>
            )}
            <button
              type="button"
              className="connector-button secondary"
              disabled={busy !== null}
              onClick={() => edit(connection)}
            >
              {t("connectors.edit")}
            </button>
            <button
              type="button"
              className="connector-button secondary"
              disabled={busy !== null || connection.status !== "ready"}
              onClick={() =>
                run(connection.id, async () => {
                  const result = await connectorApi.test(connection.id);
                  setTests((old) => ({ ...old, [connection.id]: result }));
                })
              }
            >
              {t("connectors.test")}
            </button>
            <button
              type="button"
              className="connector-button danger"
              disabled={busy !== null}
              onClick={() =>
                run(connection.id, () => connectorApi.disconnect(connection.id))
              }
            >
              {t("connectors.disconnect")}
            </button>
          </div>
          {tests[connection.id] && (
            <p role="status">
              {tests[connection.id].readable
                ? t("connectors.readable")
                : t("connectors.notReadable")}{" "}
              · {t("connectors.publicationUnknown")}
            </p>
          )}
          {connection.kind === "azure_devops" &&
            connection.status === "ready" && (
              <AzureDefaults
                key={`${connection.id}:${connection.policy_revision}`}
                connection={connection}
                onSaved={(config) =>
                  setConnections((old) =>
                    old.map((item) => (item.id === config.id ? config : item)),
                  )
                }
              />
            )}
          {connection.kind === "notion" &&
            connection.default_notion_page_id && (
              <p className="connector-help">
                {t("connectors.defaultPage")}:{" "}
                {connection.default_notion_page_id}
              </p>
            )}
        </section>
      ))}
      <McpClientSettings connections={connections} />
    </div>
  );
}
