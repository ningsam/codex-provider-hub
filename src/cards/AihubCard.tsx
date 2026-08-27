import { useCallback, useEffect, useState, type FormEvent } from "react";
import { CardShell } from "../components/CardShell";
import { ProgressBar } from "../components/ProgressBar";
import { useI18n } from "../i18n";
import { api, REFRESH_MS } from "../lib/api";
import type { AihubBalance } from "../types";

export function AihubCard() {
  const { t } = useI18n();
  const [data, setData] = useState<AihubBalance | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [showKeyForm, setShowKeyForm] = useState(false);
  const [apiKey, setApiKey] = useState("");

  const refresh = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      setData(await api.getAihubBalance());
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    const boot = window.setTimeout(() => void refresh(), 400);
    const id = window.setInterval(() => void refresh(), REFRESH_MS.aihub);
    return () => {
      window.clearTimeout(boot);
      window.clearInterval(id);
    };
  }, [refresh]);

  const onSaveKey = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const balance = await api.setAihubApiKey(apiKey);
      setData(balance);
      setApiKey("");
      setShowKeyForm(false);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const onClearKey = async () => {
    if (!window.confirm(t("aihub.clearConfirm"))) return;
    setBusy(true);
    setError(null);
    try {
      await api.clearAihubApiKey();
      setData(await api.getAihubBalance());
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const total = data ? data.balance + data.used : 0;
  const remainingPct = data && total > 0 ? (data.balance / total) * 100 : data ? 100 : 0;

  return (
    <CardShell
      title={t("aihub.title")}
      subtitle={t("aihub.subtitle")}
      refreshedAt={data?.fetchedAt}
      onRefresh={() => void refresh()}
      refreshing={busy}
      actions={
        <>
          <button
            type="button"
            className="btn ghost"
            disabled={busy}
            onClick={() => setShowKeyForm((value) => !value)}
          >
            {showKeyForm ? t("common.cancel") : t("aihub.setKey")}
          </button>
          {data?.hasStoredKey ? (
            <button
              type="button"
              className="btn ghost danger-text"
              disabled={busy}
              onClick={() => void onClearKey()}
            >
              {t("aihub.clearKey")}
            </button>
          ) : null}
        </>
      }
    >
      {error ? <p className="error-line">{error}</p> : null}

      {showKeyForm ? (
        <form className="provider-form" onSubmit={(event) => void onSaveKey(event)}>
          <label>
            {t("aihub.apiKey")}
            <input
              type="password"
              value={apiKey}
              onChange={(event) => setApiKey(event.target.value)}
              placeholder="sk-…"
              required
              autoComplete="off"
              spellCheck={false}
            />
          </label>
          <div className="form-actions">
            <button type="submit" className="btn primary" disabled={busy || !apiKey.trim()}>
              {t("aihub.saveValidate")}
            </button>
          </div>
          <p className="form-note">{t("aihub.keyNote")}</p>
        </form>
      ) : null}

      <div className="metric-row">
        <div>
          <div className="metric-label">{t("aihub.balance")}</div>
          <div className="metric-value">
            {data ? (
              <>
                <span className="mono">{data.balance.toFixed(2)}</span>
                <span className="metric-unit">{data.currency}</span>
              </>
            ) : (
              "—"
            )}
          </div>
        </div>
        <div>
          <div className="metric-label">{t("aihub.usedToday")}</div>
          <div className="metric-value mono">{data ? data.used.toFixed(2) : "—"}</div>
        </div>
      </div>
      {data ? (
        <ProgressBar value={remainingPct} invertTone label={t("aihub.balanceRatio")} />
      ) : null}
      {data?.keySource ? (
        <p className="muted-line mono">{t("aihub.source", { source: data.keySource })}</p>
      ) : null}
    </CardShell>
  );
}
