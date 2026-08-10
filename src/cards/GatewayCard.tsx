import { useCallback, useEffect, useState } from "react";
import { CardShell } from "../components/CardShell";
import { api, REFRESH_MS } from "../lib/api";
import type { GatewayStatus } from "../types";

export function GatewayCard() {
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
      title="本地网关"
      subtitle="127.0.0.1:18080 · Docker Sub2API"
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
          {running ? "Stop" : "Start"}
        </button>
      }
    >
      {error ? <p className="error-line">{error}</p> : null}
      <div className="metric-row">
        <div>
          <div className="metric-label">状态</div>
          <div className={`metric-value status ${running ? "is-on" : "is-off"}`}>
            {running ? "running" : "stopped"}
          </div>
        </div>
        <div>
          <div className="metric-label">健康</div>
          <div className="metric-value">
            {status ? (status.healthy ? "healthy" : "unhealthy") : "—"}
          </div>
        </div>
      </div>
      <dl className="kv-grid">
        <div>
          <dt>端口</dt>
          <dd className="mono">{status?.port ?? "—"}</dd>
        </div>
        <div>
          <dt>供应商</dt>
          <dd className="mono">{status?.providerCount ?? "—"}</dd>
        </div>
        <div>
          <dt>模型路由</dt>
          <dd className="mono">{status?.modelCount ?? "—"}</dd>
        </div>
      </dl>
    </CardShell>
  );
}
