import { useCallback, useEffect, useState, type FormEvent } from "react";
import { CardShell } from "../components/CardShell";
import { ProgressBar } from "../components/ProgressBar";
import { api, REFRESH_MS } from "../lib/api";
import type { CursorAccount, CursorUsage } from "../types";

export function CursorPoolCard() {
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
        list.map(async (a) => {
          try {
            const usage = await api.getCursorUsage(a.id);
            return { id: a.id, usage, error: null as string | null };
          } catch (e) {
            return {
              id: a.id,
              usage: null as CursorUsage | null,
              error: e instanceof Error ? e.message : String(e),
            };
          }
        }),
      );
      const map: Record<string, CursorUsage> = {};
      const errs: Record<string, string> = {};
      for (const entry of entries) {
        if (entry.usage) map[entry.id] = entry.usage;
        if (entry.error) errs[entry.id] = entry.error;
      }
      setUsageById(map);
      setErrById(errs);
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

  const onAdd = async (e: FormEvent) => {
    e.preventDefault();
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
    .map((u) => u.fetchedAt)
    .sort();
  const latest = fetched.length > 0 ? fetched[fetched.length - 1] : null;

  return (
    <CardShell
      className="card-span-2"
      index="06"
      title="Cursor 多账号池"
      subtitle="Per-account plan usage · tokens encrypted at rest"
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
            导入本机
          </button>
          <button
            type="button"
            className="btn primary"
            onClick={() => setShowForm((v) => !v)}
          >
            {showForm ? "Cancel" : "添加账号"}
          </button>
        </>
      }
    >
      {error ? <p className="error-line">{error}</p> : null}

      {showForm ? (
        <form className="add-form" onSubmit={(e) => void onAdd(e)}>
          <label>
            Email
            <input
              value={email}
              onChange={(e) => setEmail(e.target.value)}
              placeholder="you@example.com（可留空）"
              autoComplete="off"
            />
          </label>
          <label>
            Access Token
            <input
              value={token}
              onChange={(e) => setToken(e.target.value)}
              placeholder="cursorAuth/accessToken JWT"
              autoComplete="off"
              required
            />
          </label>
          <button type="submit" className="btn primary" disabled={busy}>
            Save
          </button>
        </form>
      ) : null}

      <div className="account-list">
        {accounts.length === 0 ? (
          <p className="empty-hint">
            还没有 Cursor 账号。点「导入本机」读取当前登录，或「添加账号」粘贴 token。
          </p>
        ) : (
          accounts.map((account) => {
            const usage = usageById[account.id];
            const accErr = errById[account.id];
            return (
              <article key={account.id} className="account-row">
                <div className="account-top">
                  <div>
                    <div className="account-email">{account.email}</div>
                    <div className="account-plan">
                      {usage?.planName ?? (accErr ? "—" : "…")} · remaining{" "}
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
                    删除
                  </button>
                </div>
                {accErr ? (
                  <p className="error-line">
                    {accErr.includes("失效") || accErr.includes("401")
                      ? "重新登录 / 更新 token"
                      : accErr}
                  </p>
                ) : null}
                {usage ? (
                  <>
                    <div className="metric-row compact">
                      <div>
                        <div className="metric-label">已用</div>
                        <div className="metric-value sm mono">
                          ${usage.used.toFixed(2)}
                        </div>
                      </div>
                      <div>
                        <div className="metric-label">总用量</div>
                        <div className="metric-value sm mono">
                          {usage.totalPercent.toFixed(1)}%
                        </div>
                      </div>
                    </div>
                    <ProgressBar value={usage.totalPercent} label="Total" />
                    <ProgressBar value={usage.autoPercent} label="Auto" />
                    <ProgressBar value={usage.apiPercent} label="API" />
                  </>
                ) : !accErr ? (
                  <p className="empty-hint">Loading usage…</p>
                ) : null}
              </article>
            );
          })
        )}
      </div>
    </CardShell>
  );
}
