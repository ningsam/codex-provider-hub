import { useCallback, useEffect, useMemo, useState } from "react";
import { CardShell } from "../components/CardShell";
import { useI18n } from "../i18n";
import { api, REFRESH_MS } from "../lib/api";
import type { GatewayStatus, RoutingState, RoutingTarget } from "../types";

const routeCopy = {
  en: {
    title: "Current route",
    target: "Route target",
    current: "Current",
    provider: "Codex provider",
    official: "OpenAI official · current Codex login",
    pool: "Sub2API OAuth pool · automatic scheduling",
    oauth: "Official account",
    relay: "Third-party relay",
    switch: "Switch route",
    switched: "Now using",
    restart:
      "New Codex sessions use this route. Restart an already-running Codex/ChatGPT session if it keeps the previous provider.",
    gatewayDown:
      "Sub2API is unavailable. The native OpenAI official route can still be selected.",
    unmanaged: "Mixed / external routing",
  },
  "zh-CN": {
    title: "当前线路",
    target: "切换目标",
    current: "当前",
    provider: "Codex Provider",
    official: "OpenAI 官方 · 当前 Codex 登录",
    pool: "Sub2API OAuth 号池 · 自动调度",
    oauth: "官方账号",
    relay: "第三方中转",
    switch: "切换线路",
    switched: "已切换到",
    restart:
      "新建 Codex 会话会使用这条线路；如果正在运行的 Codex/ChatGPT 仍缓存旧 Provider，请重启该会话或应用。",
    gatewayDown: "Sub2API 当前不可用；仍然可以切换到 OpenAI 官方线路。",
    unmanaged: "混合 / 外部路由",
  },
  ja: {
    title: "現在のルート",
    target: "切り替え先",
    current: "現在",
    provider: "Codex Provider",
    official: "OpenAI 公式 · 現在の Codex ログイン",
    pool: "Sub2API OAuth プール · 自動スケジュール",
    oauth: "公式アカウント",
    relay: "サードパーティリレー",
    switch: "ルートを切り替え",
    switched: "切り替え先",
    restart:
      "新しい Codex セッションはこのルートを使用します。実行中の Codex/ChatGPT が以前の Provider を保持する場合は再起動してください。",
    gatewayDown: "Sub2API は利用できませんが、OpenAI 公式ルートには切り替えられます。",
    unmanaged: "混在 / 外部ルーティング",
  },
} as const;

export function GatewayCard() {
  const { locale, t } = useI18n();
  const copy = routeCopy[locale] ?? routeCopy.en;
  const [status, setStatus] = useState<GatewayStatus | null>(null);
  const [routing, setRouting] = useState<RoutingState | null>(null);
  const [routeChoice, setRouteChoice] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [routeError, setRouteError] = useState<string | null>(null);
  const [routeHint, setRouteHint] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [routeBusy, setRouteBusy] = useState(false);

  const targetLabel = useCallback(
    (target: RoutingTarget) => {
      if (target.kind === "official") return copy.official;
      if (target.kind === "pool") return copy.pool;
      if (target.kind === "oauth") return `${copy.oauth} · ${target.name}`;
      return `${copy.relay} · ${target.name}`;
    },
    [copy],
  );

  const refresh = useCallback(async () => {
    setBusy(true);
    setError(null);
    const [gatewayResult, routingResult] = await Promise.allSettled([
      api.getGatewayStatus(),
      api.getRoutingState(),
    ]);

    if (gatewayResult.status === "fulfilled") {
      setStatus(gatewayResult.value);
    } else {
      const e = gatewayResult.reason;
      setError(e instanceof Error ? e.message : String(e));
    }

    if (routingResult.status === "fulfilled") {
      const next = routingResult.value;
      setRouting(next);
      setRouteError(null);
      setRouteChoice((current) => {
        if (current && next.targets.some((target) => target.id === current)) return current;
        if (next.activeTarget !== "unmanaged") return next.activeTarget;
        return next.targets.find((target) => target.available)?.id ?? "official";
      });
    } else {
      const e = routingResult.reason;
      setRouteError(e instanceof Error ? e.message : String(e));
    }
    setBusy(false);
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
      const nextRouting = await api.getRoutingState();
      setRouting(nextRouting);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const switchRoute = async () => {
    if (!routeChoice) return;
    setRouteBusy(true);
    setRouteError(null);
    setRouteHint(null);
    try {
      const next = await api.switchRoutingTarget(routeChoice);
      setRouting(next);
      setRouteChoice(next.activeTarget === "unmanaged" ? routeChoice : next.activeTarget);
      const active = next.targets.find((target) => target.selected);
      setRouteHint(`${copy.switched} ${active ? targetLabel(active) : routeChoice}`);
      setStatus(await api.getGatewayStatus());
    } catch (e) {
      setRouteError(e instanceof Error ? e.message : String(e));
    } finally {
      setRouteBusy(false);
    }
  };

  const running = status?.running ?? false;
  const activeTarget = routing?.targets.find((target) => target.selected) ?? null;
  const selectedTarget = routing?.targets.find((target) => target.id === routeChoice) ?? null;
  const activeLabel = useMemo(
    () => (activeTarget ? targetLabel(activeTarget) : copy.unmanaged),
    [activeTarget, copy.unmanaged, targetLabel],
  );

  return (
    <CardShell
      index="01"
      title={t("gateway.title")}
      subtitle={t("gateway.subtitle")}
      refreshedAt={status?.lastCheckedAt}
      onRefresh={() => void refresh()}
      refreshing={busy || routeBusy}
      actions={
        <button
          type="button"
          className={running ? "btn danger" : "btn primary"}
          onClick={() => void toggle()}
          disabled={busy || routeBusy || !status}
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

      <div className="provider-form">
        <label>
          {copy.title}
          <select
            value={routeChoice}
            disabled={routeBusy || !routing}
            onChange={(event) => {
              setRouteChoice(event.target.value);
              setRouteHint(null);
              setRouteError(null);
            }}
          >
            {routing?.targets.map((target) => (
              <option key={target.id} value={target.id} disabled={!target.available}>
                {targetLabel(target)}{target.detail ? ` — ${target.detail}` : ""}
              </option>
            ))}
          </select>
        </label>
        <div>
          <div className="metric-label">{copy.current}</div>
          <div className="metric-value" style={{ fontSize: "0.76rem", lineHeight: 1.35 }}>
            {routing ? activeLabel : "—"}
          </div>
          <div className="provider-meta mono">
            {copy.provider}: {routing?.modelProvider ?? "—"}
          </div>
        </div>
        <div className="form-actions">
          <button
            type="button"
            className="btn primary"
            disabled={
              routeBusy ||
              !routing ||
              !selectedTarget?.available ||
              routeChoice === routing.activeTarget
            }
            onClick={() => void switchRoute()}
          >
            {copy.switch}
          </button>
        </div>
        <p className="form-note">{copy.restart}</p>
      </div>

      {routing?.gatewayError ? <p className="warn-line">{copy.gatewayDown}</p> : null}
      {routeError ? <p className="error-line">{routeError}</p> : null}
      {routeHint ? <p className="hint-line">{routeHint}</p> : null}
    </CardShell>
  );
}
