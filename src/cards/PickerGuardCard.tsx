import { useCallback, useEffect, useState } from "react";
import { CardShell } from "../components/CardShell";
import { useI18n } from "../i18n";
import { api, REFRESH_MS } from "../lib/api";
import type { PickerGuardStatus } from "../types";

export function PickerGuardCard() {
  const { t } = useI18n();
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
        // Keep the original operation error.
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
    hidden === true
      ? t("guard.hiddenTrue")
      : hidden === false
        ? "false"
        : t("common.notFound");
  const unguarded = !!status?.chatgptRunning && status.hostRulesActive === false;
  const healthy =
    !!status?.enabled &&
    hidden !== true &&
    (!status.chatgptRunning || status.hostRulesActive);

  return (
    <CardShell
      index="02"
      title={t("guard.title")}
      titleBadge={
        <span
          className={`guard-title-badge ${healthy ? "is-ok" : "is-danger"}`}
          title={healthy ? t("guard.protectedTitle") : t("guard.repairTitle")}
        >
          {healthy ? t("guard.protected") : t("guard.needsRepair")}
        </span>
      }
      subtitle={t("guard.subtitle")}
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
            {status?.enabled ? t("guard.disable") : t("guard.enable")}
          </button>
          <button
            type="button"
            className="btn primary"
            onClick={() => void apply()}
            disabled={busy}
          >
            {t("guard.apply")}
          </button>
        </>
      }
    >
      {error ? <p className="error-line">{error}</p> : null}
      {unguarded ? <p className="guard-alert">{t("guard.unguarded")}</p> : null}
      {status?.pendingFix && !unguarded ? (
        <p className="warn-line">{t("guard.pending")}</p>
      ) : null}
      {status?.lastError && !status.pendingFix && !unguarded ? (
        <p className="warn-line">{status.lastError}</p>
      ) : null}
      <div className="metric-row">
        <div>
          <div className="metric-label">{t("guard.metric")}</div>
          <div className={`metric-value status ${status?.enabled ? "is-on" : "is-off"}`}>
            {status?.enabled ? t("common.on") : t("common.off")}
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
          <dd className="mono">{status?.chatgptRunning ? t("common.running") : t("common.stopped")}</dd>
        </div>
        <div>
          <dt>host-rules</dt>
          <dd
            className={`mono ${
              status?.hostRulesActive
                ? "is-ok-text"
                : status?.chatgptRunning
                  ? "is-danger-text"
                  : ""
            }`}
          >
            {!status?.chatgptRunning
              ? "—"
              : status.hostRulesActive
                ? t("common.active")
                : t("common.missingStatus")}
          </dd>
        </div>
        <div>
          <dt>{t("guard.lastPatch")}</dt>
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
          {t("guard.openProtected")}
        </button>
      </div>
    </CardShell>
  );
}
