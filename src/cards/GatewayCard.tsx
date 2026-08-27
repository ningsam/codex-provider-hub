import { useCallback, useEffect, useState } from "react";
import { CardShell } from "../components/CardShell";
import { useI18n } from "../i18n";
import { api, REFRESH_MS } from "../lib/api";
import type { GatewayStatus } from "../types";

export function GatewayCard() {
  const { t } = useI18n();
  const [status, setStatus] = useState<GatewayStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      setStatus(await api.getGatewayStatus());
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
    const id = window.setInterval(() => void refresh(), REFRESH_MS.gateway);
    return () => window.clearInterval(id);
  }, [refresh]);

  const toggle = async () => {
    if (!status) return;
    setBusy(true);
    setError(null);
    try {
      setStatus(status.running ? await api.stopGateway() : await api.startGateway());
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const running = status?.running ?? false;

  return (
    <CardShell
      index="01"
      title={t("gateway.title")}
      subtitle={t("gateway.subtitle")}
      refreshedAt={status?.lastCheckedAt}
      onRefresh={() => void refresh()}
      refreshing={busy}
      actions={
        <button
          type="button"
          className={running ? "btn danger" : "btn primary"}
          onClick={() => void toggle()}
          disabled={busy || !status}
        >
          {running ? t("gateway.stop") : t("gateway.start")}
        </button>
      }
    >
      {error ? <p className="error-line">{error}</p> : null}
      <div className="metric-row">
        <div>
          <div className="metric-label">{t("gateway.status")}</div>
          <div className={`metric-value status ${running ? "is-on" : "is-off"}`}>
            {running ? t("common.running") : t("common.stopped")}
          </div>
        </div>
        <div>
          <div className="metric-label">{t("gateway.health")}</div>
          <div className="metric-value">
            {status ? (status.healthy ? t("common.healthy") : t("common.unhealthy")) : "—"}
          </div>
        </div>
      </div>
      <dl className="kv-grid">
        <div>
          <dt>{t("gateway.port")}</dt>
          <dd className="mono">{status?.port ?? "—"}</dd>
        </div>
        <div>
          <dt>{t("gateway.providers")}</dt>
          <dd className="mono">{status?.providerCount ?? "—"}</dd>
        </div>
        <div>
          <dt>{t("gateway.routes")}</dt>
          <dd className="mono">{status?.modelCount ?? "—"}</dd>
        </div>
      </dl>
    </CardShell>
  );
}
