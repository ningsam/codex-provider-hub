import { useCallback, useEffect, useState } from "react";
import { CardShell } from "../components/CardShell";
import { api, REFRESH_MS } from "../lib/api";
import type { PickerGuardStatus } from "../types";

export function PickerGuardCard() {
  const [status, setStatus] = useState<PickerGuardStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      setStatus(await api.getPickerGuardStatus());
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
    const id = window.setInterval(() => void refresh(), REFRESH_MS.pickerGuard);
    return () => window.clearInterval(id);
  }, [refresh]);

  const toggle = async () => {
    if (!status) return;
    setBusy(true);
    setError(null);
    try {
      setStatus(await api.setPickerGuardEnabled(!status.enabled));
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const apply = async () => {
    setBusy(true);
    setError(null);
    try {
      setStatus(await api.applyPickerGuard());
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      try {
        setStatus(await api.getPickerGuardStatus());
      } catch {
        /* ignore */
      }
    } finally {
      setBusy(false);
    }
  };

  const openGuarded = async () => {
    setBusy(true);
    setError(null);
    try {
      setStatus(await api.openChatgptGuarded());
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const hidden = status?.useHiddenModels;
  const hiddenLabel =
    hidden === true ? "true（会过滤自定义模型）" : hidden === false ? "false" : "未找到";
  const unguarded =
    !!status?.chatgptRunning && status.hostRulesActive === false;
  // Byte-scan can miss compressed LevelDB values; host-rules + not-true is enough.
  const healthy =
    !!status?.enabled &&
    hidden !== true &&
    (!status.chatgptRunning || status.hostRulesActive);

  return (
    <CardShell
      index="02"
      title="Codex 模型选择器守护"
      titleBadge={
        <span
          className={`guard-title-badge ${healthy ? "is-ok" : "is-danger"}`}
          title={healthy ? "已防护" : "需要修复"}
        >
          {healthy ? "已防护" : "需修复"}
        </span>
      }
      subtitle="Statsig use_hidden_models · Local Storage · host-rules"
      refreshedAt={status?.patchedAt}
      onRefresh={() => void refresh()}
      refreshing={busy}
      className="picker-guard-card"
      actions={
        <>
          <button
            type="button"
            className={status?.enabled ? "btn danger" : "btn primary"}
            onClick={() => void toggle()}
            disabled={busy || !status}
          >
            {status?.enabled ? "关闭守护" : "开启守护"}
          </button>
          <button
            type="button"
            className="btn primary"
            onClick={() => void apply()}
            disabled={busy}
          >
            立即修复并防刷新启动 ChatGPT
          </button>
        </>
      }
    >
      {error ? <p className="error-line">{error}</p> : null}
      {unguarded ? (
        <p className="guard-alert">
          当前 ChatGPT 未防刷新，点立即修复
        </p>
      ) : null}
      {status?.pendingFix && !unguarded ? (
        <p className="warn-line">ChatGPT 运行中，将在退出后自动修复</p>
      ) : null}
      {status?.lastError && !status.pendingFix && !unguarded ? (
        <p className="warn-line">{status.lastError}</p>
      ) : null}
      <div className="metric-row">
        <div>
          <div className="metric-label">守护</div>
          <div className={`metric-value status ${status?.enabled ? "is-on" : "is-off"}`}>
            {status?.enabled ? "on" : "off"}
          </div>
        </div>
        <div>
          <div className="metric-label">use_hidden_models</div>
          <div
            className={`metric-value status ${
              hidden === false ? "is-on" : hidden === true ? "is-off" : ""
            }`}
          >
            {hiddenLabel}
          </div>
        </div>
      </div>
      <dl className="kv-grid">
        <div>
          <dt>ChatGPT</dt>
          <dd className="mono">{status?.chatgptRunning ? "running" : "stopped"}</dd>
        </div>
        <div>
          <dt>host-rules</dt>
          <dd className={`mono ${status?.hostRulesActive ? "is-ok-text" : status?.chatgptRunning ? "is-danger-text" : ""}`}>
            {!status?.chatgptRunning
              ? "—"
              : status.hostRulesActive
                ? "active"
                : "MISSING"}
          </dd>
        </div>
        <div>
          <dt>上次补丁</dt>
          <dd className="mono">{status?.patchedAt ?? "—"}</dd>
        </div>
      </dl>
      <div className="card-inline-actions">
        <button
          type="button"
          className="btn ghost"
          onClick={() => void openGuarded()}
          disabled={busy}
        >
          用防刷新方式打开 ChatGPT
        </button>
      </div>
    </CardShell>
  );
}
