import { useCallback, useEffect, useState, type FormEvent } from "react";
import { CardShell } from "../components/CardShell";
import { ProgressBar } from "../components/ProgressBar";
import { useI18n } from "../i18n";
import { api, REFRESH_MS } from "../lib/api";
import type { CursorAccount, CursorUsage } from "../types";

export function CursorPoolCard() {
  const { t } = useI18n();
  const [accounts, setAccounts] = useState<CursorAccount[]>([]);
  const [usageById, setUsageById] = useState<Record<string, CursorUsage>>({});
  const [errById, setErrById] = useState<Record<string, string>>({});
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [email, setEmail] = useState("");
  const [token, setToken] = useState("");
  const [showForm, setShowForm] = useState(false);

  const refresh = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      const list = await api.listCursorAccounts();
      setAccounts(list);
      const entries = await Promise.all(
        list.map(async (account) => {
          try {
            const usage = await api.getCursorUsage(account.id);
            return { id: account.id, usage, error: null as string | null };
          } catch (e) {
            return {
              id: account.id,
              usage: null as CursorUsage | null,
              error: e instanceof Error ? e.message : String(e),
            };
          }
        }),
      );
      const usageMap: Record<string, CursorUsage> = {};
      const errorMap: Record<string, string> = {};
      for (const entry of entries) {
        if (entry.usage) usageMap[entry.id] = entry.usage;
        if (entry.error) errorMap[entry.id] = entry.error;
      }
      setUsageById(usageMap);
      setErrById(errorMap);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    const boot = window.setTimeout(() => void refresh(), 800);
    const id = window.setInterval(() => void refresh(), REFRESH_MS.cursor);
    return () => {
      window.clearTimeout(boot);
      window.clearInterval(id);
    };
  }, [refresh]);

  const onAdd = async (event: FormEvent) => {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await api.addCursorAccount(email, token);
      setEmail("");
      setToken("");
      setShowForm(false);
      await refresh();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      setBusy(false);
    }
  };

  const onImportLocal = async () => {
    setBusy(true);
    setError(null);
    try {
      await api.importLocalCursorAccount();
      setShowForm(false);
      await refresh();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      setBusy(false);
    }
  };

  const onRemove = async (id: string) => {
    setBusy(true);
    setError(null);
    try {
      await api.removeCursorAccount(id);
      await refresh();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      setBusy(false);
    }
  };

  const fetched = Object.values(usageById)
    .map((usage) => usage.fetchedAt)
    .sort();
  const latest = fetched.length > 0 ? fetched[fetched.length - 1] : null;

  return (
    <CardShell
      className="card-span-2"
      index="06"
      title={t("cursor.title")}
      subtitle={t("cursor.subtitle")}
      refreshedAt={latest}
      onRefresh={() => void refresh()}
      refreshing={busy}
      actions={
        <>
          <button
            type="button"
            className="btn ghost"
            onClick={() => void onImportLocal()}
            disabled={busy}
          >
            {t("cursor.importLocal")}
          </button>
          <button
            type="button"
            className="btn primary"
            onClick={() => setShowForm((value) => !value)}
            disabled={busy}
          >
            {showForm ? t("common.cancel") : t("cursor.addAccount")}
          </button>
        </>
      }
    >
      {error ? <p className="error-line">{error}</p> : null}

      {showForm ? (
        <form className="add-form" onSubmit={(event) => void onAdd(event)}>
          <label>
            {t("cursor.email")}
            <input
              value={email}
              onChange={(event) => setEmail(event.target.value)}
              placeholder={t("cursor.emailPlaceholder")}
              autoComplete="off"
            />
          </label>
          <label>
            {t("cursor.accessToken")}
            <input
              value={token}
              onChange={(event) => setToken(event.target.value)}
              placeholder={t("cursor.tokenPlaceholder")}
              autoComplete="off"
              required
            />
          </label>
          <button type="submit" className="btn primary" disabled={busy}>
            {t("common.save")}
          </button>
        </form>
      ) : null}

      <div className="account-list">
        {accounts.length === 0 ? (
          <p className="empty-hint">{t("cursor.empty")}</p>
        ) : (
          accounts.map((account) => {
            const usage = usageById[account.id];
            const accountError = errById[account.id];
            return (
              <article key={account.id} className="account-row">
                <div className="account-top">
                  <div>
                    <div className="account-email">{account.email}</div>
                    <div className="account-plan">
                      {usage?.planName ?? (accountError ? "—" : "…")} · {t("cursor.remaining")}{" "}
                      <span className="mono">
                        {usage ? usage.remaining.toFixed(2) : "—"}
                      </span>
                      {usage ? ` / ${usage.planLimit.toFixed(2)}` : ""}
                    </div>
                  </div>
                  <button
                    type="button"
                    className="btn ghost danger-text"
                    onClick={() => void onRemove(account.id)}
                    disabled={busy}
                  >
                    {t("common.delete")}
                  </button>
                </div>
                {accountError ? (
                  <p className="error-line">
                    {accountError.includes("失效") || accountError.includes("401")
                      ? t("cursor.relogin")
                      : accountError}
                  </p>
                ) : null}
                {usage ? (
                  <>
                    <div className="metric-row compact">
                      <div>
                        <div className="metric-label">{t("cursor.used")}</div>
                        <div className="metric-value sm mono">
                          ${usage.used.toFixed(2)}
                        </div>
                      </div>
                      <div>
                        <div className="metric-label">{t("cursor.totalUsage")}</div>
                        <div className="metric-value sm mono">
                          {usage.totalPercent.toFixed(1)}%
                        </div>
                      </div>
                    </div>
                    <ProgressBar value={usage.totalPercent} label={t("cursor.total")} />
                    <ProgressBar value={usage.autoPercent} label={t("cursor.auto")} />
                    <ProgressBar value={usage.apiPercent} label={t("cursor.api")} />
                  </>
                ) : !accountError ? (
                  <p className="empty-hint">{t("cursor.loadingUsage")}</p>
                ) : null}
              </article>
            );
          })
        )}
      </div>
    </CardShell>
  );
}
