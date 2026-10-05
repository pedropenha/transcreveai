import React, { useCallback, useEffect, useMemo, useState } from "react";
import { ExternalLink, Plus, Sparkles } from "lucide-react";
import { useTranslation } from "react-i18next";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  connectorApi,
  type AzureCatalog,
  type AzureWorkItemDraft,
  type ConnectorConfig,
  type ItemSuggestion,
  type OperationRecord,
  type RemoteDocument,
} from "../settings/connectors/api";
import "../settings/connectors/connectors.css";

const blankDraft = (projectId: string): AzureWorkItemDraft => ({
  project_id: projectId,
  work_item_type: "",
  title: "",
  description: "",
  area_path: null,
  iteration_path: null,
  parent_id: null,
});

const errorMessage = (error: unknown): string => {
  if (typeof error === "object" && error !== null && "code" in error) {
    const code = (error as { code: unknown }).code;
    if (typeof code === "string") return code;
  }
  return "temporary_failure";
};

/**
 * Etapa 2 (Azure): suggest work items from the meeting summary and create
 * them after an explicit, fingerprint-bound review. The assistant itself can
 * never write — creation only happens through this reviewed path.
 */
export const MeetingConnectorActions: React.FC<{
  meetingId: string;
  hasSummary: boolean;
}> = ({ meetingId, hasSummary }) => {
  const { t } = useTranslation();
  const [connections, setConnections] = useState<ConnectorConfig[]>([]);
  const [operations, setOperations] = useState<OperationRecord[]>([]);
  const [suggestions, setSuggestions] = useState<ItemSuggestion[] | null>(null);
  const [editing, setEditing] = useState<AzureWorkItemDraft | null>(null);
  const [catalog, setCatalog] = useState<AzureCatalog | null>(null);
  const [parents, setParents] = useState<RemoteDocument[]>([]);
  const [prepared, setPrepared] = useState<{
    operation_id: string;
    fingerprint: string;
    warnings: string[];
  } | null>(null);
  const [connectionId, setConnectionId] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    const [all, ops] = await Promise.all([
      connectorApi.list(),
      connectorApi.listMeetingOperations(meetingId),
    ]);
    const azure = all.filter(
      (c) => c.kind === "azure_devops" && c.status === "ready",
    );
    setConnections(azure);
    setOperations(ops);
    setConnectionId((old) =>
      azure.some((c) => c.id === old) ? old : (azure[0]?.id ?? ""),
    );
  }, [meetingId]);

  useEffect(() => {
    void refresh().catch(() => setError("temporary_failure"));
  }, [refresh]);

  const connection = useMemo(
    () => connections.find((c) => c.id === connectionId) ?? null,
    [connections, connectionId],
  );

  const allowedProjects = useMemo(
    () =>
      (catalog?.projects ?? []).filter((p) =>
        (connection?.scope.azure_project_ids ?? []).includes(p.id),
      ),
    [catalog, connection],
  );

  const openForm = useCallback(
    (initial?: Partial<AzureWorkItemDraft>) => {
      setPrepared(null);
      setError(null);
      const projectId = connection?.scope.azure_project_ids[0] ?? "";
      setEditing({ ...blankDraft(projectId), ...initial });
      setParents([]);
      setCatalog(null);
      if (connection && projectId) {
        connectorApi
          .catalog(connection.id, projectId)
          .then(setCatalog)
          .catch((e: unknown) => setError(errorMessage(e)));
        connectorApi
          .query(connection.id, projectId)
          .then(setParents)
          .catch(() => setParents([]));
      }
    },
    [connection],
  );

  const suggest = async () => {
    if (!connection) return;
    setBusy(true);
    setError(null);
    try {
      setSuggestions(
        await connectorApi.suggestAzureItems(connection.id, meetingId),
      );
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const prepare = async () => {
    if (!connection || !editing) return;
    setBusy(true);
    setError(null);
    try {
      const result = await connectorApi.prepareAzureItem(
        connection.id,
        meetingId,
        editing,
      );
      setPrepared({
        operation_id: result.operation_id,
        fingerprint: result.fingerprint,
        warnings: result.warnings,
      });
      setEditing(result.draft);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const execute = async () => {
    if (!prepared) return;
    setBusy(true);
    setError(null);
    try {
      await connectorApi.executeAction(
        prepared.operation_id,
        prepared.fingerprint,
      );
      setEditing(null);
      setPrepared(null);
      setSuggestions(null);
      await refresh();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  if (connections.length === 0) return null;

  return (
    <section className="nt-sec" aria-label={t("connectors.meeting.title")}>
      <h3 className="caps">{t("connectors.meeting.title")}</h3>

      {operations.length > 0 ? (
        <ul className="nt-ops">
          {operations.map((op) => (
            <li key={op.id} className="nt-op">
              <span>
                {t(`connectors.meeting.op.${op.status}`, {
                  defaultValue: op.status,
                })}
              </span>
              {op.remote_url ? (
                <button
                  type="button"
                  className="link-button"
                  onClick={() => void openUrl(op.remote_url!)}
                >
                  {t("connectors.meeting.openItem", { id: op.remote_id })}
                  <ExternalLink width={12} height={12} aria-hidden="true" />
                </button>
              ) : null}
            </li>
          ))}
        </ul>
      ) : null}

      <div className="nt-acts">
        <button
          type="button"
          className="btn-secondary"
          disabled={busy || !hasSummary}
          title={hasSummary ? undefined : t("connectors.meeting.needsSummary")}
          onClick={() => void suggest()}
        >
          <Sparkles width={14} height={14} aria-hidden="true" />
          {t("connectors.meeting.suggest")}
        </button>
        <button
          type="button"
          className="btn-secondary"
          disabled={busy}
          onClick={() => openForm()}
        >
          <Plus width={14} height={14} aria-hidden="true" />
          {t("connectors.meeting.create")}
        </button>
      </div>

      {suggestions && suggestions.length === 0 ? (
        <p className="nt-muted">{t("connectors.meeting.noSuggestions")}</p>
      ) : null}
      {suggestions?.map((s, i) => (
        <article className="connector-card nt-suggestion" key={i}>
          <h4>{s.title}</h4>
          <p className="connector-help">
            {s.work_item_type} · {s.rationale}
          </p>
          {s.source_excerpt ? (
            <blockquote className="nt-excerpt">“{s.source_excerpt}”</blockquote>
          ) : null}
          <button
            type="button"
            className="btn-secondary"
            disabled={busy}
            onClick={() =>
              openForm({
                title: s.title,
                description: s.description,
                work_item_type: s.work_item_type,
              })
            }
          >
            {t("connectors.meeting.review")}
          </button>
        </article>
      ))}

      {editing ? (
        <div className="connector-card nt-editor">
          {connections.length > 1 ? (
            <label className="connector-field">
              <span>{t("connectors.meeting.connection")}</span>
              <select
                value={connectionId}
                onChange={(e) => setConnectionId(e.target.value)}
              >
                {connections.map((c) => (
                  <option key={c.id} value={c.id}>
                    {c.label}
                  </option>
                ))}
              </select>
            </label>
          ) : null}
          <label className="connector-field">
            <span>{t("connectors.meeting.project")}</span>
            <select
              value={editing.project_id}
              onChange={(e) =>
                setEditing({ ...editing, project_id: e.target.value })
              }
            >
              {allowedProjects.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name}
                </option>
              ))}
              {allowedProjects.length === 0 &&
              connection?.scope.azure_project_ids.length ? (
                connection.scope.azure_project_ids.map((id) => (
                  <option key={id} value={id}>
                    {id}
                  </option>
                ))
              ) : allowedProjects.length === 0 ? (
                <option value="">
                  {t("connectors.meeting.noAllowedProjects")}
                </option>
              ) : null}
            </select>
          </label>
          <label className="connector-field">
            <span>{t("connectors.meeting.itemType")}</span>
            <select
              value={editing.work_item_type}
              onChange={(e) =>
                setEditing({ ...editing, work_item_type: e.target.value })
              }
            >
              <option value="">{t("connectors.choose")}</option>
              {(catalog?.work_item_types ?? []).map((entry) => (
                <option key={entry.id} value={entry.name}>
                  {entry.name}
                </option>
              ))}
            </select>
          </label>
          <label className="connector-field">
            <span>{t("connectors.meeting.itemTitle")}</span>
            <input
              value={editing.title}
              maxLength={256}
              onChange={(e) =>
                setEditing({ ...editing, title: e.target.value })
              }
            />
          </label>
          <label className="connector-field">
            <span>{t("connectors.meeting.description")}</span>
            <textarea
              rows={5}
              value={editing.description}
              onChange={(e) =>
                setEditing({ ...editing, description: e.target.value })
              }
            />
          </label>
          <label className="connector-field">
            <span>{t("connectors.meeting.parent")}</span>
            <select
              value={editing.parent_id ?? ""}
              onChange={(e) =>
                setEditing({
                  ...editing,
                  parent_id: e.target.value ? Number(e.target.value) : null,
                })
              }
            >
              <option value="">{t("connectors.meeting.noParent")}</option>
              {parents.map((p) => (
                <option key={p.id} value={p.id}>
                  #{p.id} — {p.title}
                </option>
              ))}
            </select>
          </label>
          {(catalog?.areas.length ?? 0) > 0 ? (
            <label className="connector-field">
              <span>{t("connectors.meeting.area")}</span>
              <select
                value={editing.area_path ?? ""}
                onChange={(e) =>
                  setEditing({
                    ...editing,
                    area_path: e.target.value || null,
                  })
                }
              >
                <option value="">{t("connectors.choose")}</option>
                {catalog!.areas.map((entry) =>
                  entry.path ? (
                    <option key={entry.id} value={entry.path}>
                      {entry.path}
                    </option>
                  ) : null,
                )}
              </select>
            </label>
          ) : null}
          {prepared?.warnings.includes("iteration_unverified") ? (
            <p className="connector-help">
              {t("connectors.meeting.iterationUnverified")}
            </p>
          ) : null}
          <div className="nt-acts">
            {prepared ? (
              <button
                type="button"
                className="connector-button"
                disabled={busy}
                onClick={() => void execute()}
              >
                {t("connectors.meeting.confirmCreate")}
              </button>
            ) : (
              <button
                type="button"
                className="connector-button"
                disabled={
                  busy ||
                  !editing.project_id ||
                  !editing.work_item_type ||
                  !editing.title.trim()
                }
                onClick={() => void prepare()}
              >
                {t("connectors.meeting.reviewCreate")}
              </button>
            )}
            <button
              type="button"
              className="link-button"
              disabled={busy}
              onClick={() => {
                setEditing(null);
                setPrepared(null);
              }}
            >
              {t("common.cancel")}
            </button>
          </div>
        </div>
      ) : null}

      {error ? (
        <p role="alert" className="connector-error">
          {t(`connectors.errors.${error}`, {
            defaultValue: t("connectors.errors.temporary_failure"),
          })}
        </p>
      ) : null}
    </section>
  );
};
