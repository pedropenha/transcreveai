import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import type { ConnectorConfig } from "./api";

interface McpGrant {
  client_id: string;
  label: string;
  connection_ids: string[];
  expires_at: number;
  revoked: boolean;
}
interface McpStatus {
  running: boolean;
  grants: McpGrant[];
}
interface McpConfig {
  command: string;
  args: string[];
  transport: "stdio";
}

export function McpClientSettings({
  connections,
}: {
  connections: ConnectorConfig[];
}) {
  const { t } = useTranslation();
  const [status, setStatus] = useState<McpStatus | null>(null);
  const [label, setLabel] = useState("");
  const [selected, setSelected] = useState<string[]>([]);
  const [config, setConfig] = useState<McpConfig | null>(null);
  const [error, setError] = useState(false);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    let active = true;
    invoke<McpStatus>("mcp_status")
      .then((value) => {
        if (active) setStatus(value);
      })
      .catch(() => {
        if (active) setError(true);
      });
    return () => {
      active = false;
    };
  }, []);
  const pair = async () => {
    setBusy(true);
    setError(false);
    try {
      const grant = await invoke<McpGrant>("mcp_pair", {
        label: label.trim(),
        connectionIds: selected,
      });
      const exported = await invoke<McpConfig>("mcp_config", {
        clientId: grant.client_id,
      });
      setConfig(exported);
      setStatus(await invoke<McpStatus>("mcp_status"));
      setLabel("");
      setSelected([]);
    } catch {
      setError(true);
    } finally {
      setBusy(false);
    }
  };
  const revoke = async (clientId: string) => {
    setBusy(true);
    setError(false);
    try {
      await invoke<void>("mcp_revoke", { clientId });
      setStatus(await invoke<McpStatus>("mcp_status"));
      setConfig(null);
    } catch {
      setError(true);
    } finally {
      setBusy(false);
    }
  };
  const exportConfig = async (clientId: string) => {
    setError(false);
    try {
      setConfig(await invoke<McpConfig>("mcp_config", { clientId }));
    } catch {
      setError(true);
    }
  };
  return (
    <section className="connector-card" aria-label={t("connectors.mcp.title")}>
      <h2>{t("connectors.mcp.title")}</h2>
      <p className="connector-help">{t("connectors.mcp.explanation")}</p>
      {status && (
        <p role="status">
          {status.running
            ? t("connectors.mcp.running")
            : t("connectors.mcp.stopped")}
        </p>
      )}
      <label className="connector-field">
        <span>{t("connectors.mcp.clientName")}</span>
        <input
          value={label}
          onChange={(event) => setLabel(event.target.value)}
          maxLength={120}
        />
      </label>
      <fieldset className="connector-scope">
        <legend>{t("connectors.mcp.allowedConnections")}</legend>
        {connections
          .filter((item) => item.status === "ready")
          .map((item) => (
            <label key={item.id}>
              <input
                type="checkbox"
                checked={selected.includes(item.id)}
                onChange={(event) =>
                  setSelected((old) =>
                    event.target.checked
                      ? [...old, item.id]
                      : old.filter((id) => id !== item.id),
                  )
                }
              />
              {item.label}
            </label>
          ))}
      </fieldset>
      <button
        type="button"
        className="connector-button"
        disabled={busy || !label.trim() || selected.length === 0}
        onClick={pair}
      >
        {t("connectors.mcp.pair")}
      </button>
      {status?.grants
        .filter((grant) => !grant.revoked)
        .map((grant) => (
          <div className="connector-grant" key={grant.client_id}>
            <span>{grant.label}</span>
            <button
              type="button"
              className="connector-button secondary"
              disabled={busy}
              onClick={() => exportConfig(grant.client_id)}
            >
              {t("connectors.mcp.showConfig")}
            </button>
            <button
              type="button"
              className="connector-button danger"
              disabled={busy}
              onClick={() => revoke(grant.client_id)}
            >
              {t("connectors.mcp.revoke")}
            </button>
          </div>
        ))}
      {config && (
        <div className="connector-config">
          <p>{t("connectors.mcp.manualSetup")}</p>
          <pre>
            {JSON.stringify(
              { command: config.command, args: config.args },
              null,
              2,
            )}
          </pre>
          <button
            type="button"
            className="connector-button secondary"
            onClick={() =>
              navigator.clipboard
                .writeText(
                  JSON.stringify(
                    { command: config.command, args: config.args },
                    null,
                    2,
                  ),
                )
                .catch(() => setError(true))
            }
          >
            {t("connectors.mcp.copy")}
          </button>
        </div>
      )}
      {error && (
        <p role="alert" className="connector-error">
          {t("connectors.mcp.error")}
        </p>
      )}
    </section>
  );
}
