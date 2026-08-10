import { useCallback, useEffect, useState } from "react";
import { CardShell } from "../components/CardShell";
import { ProgressBar } from "../components/ProgressBar";
import { api, REFRESH_MS } from "../lib/api";
import { formatDuration } from "../lib/format";
import type { Sub2ApiAccountQuota, Sub2ApiUsage } from "../types";

function statusLabel(status: string): string {
  switch (status) {
    case "ready":
      return "可用";
    case "error":
      return "异常/封禁";
    case "inactive":
      return "停用";
    default:
      return status || "未知";
  }
}

function AccountMiniCard({
  account,
  busy,
  onDelete,
}: {
  account: Sub2ApiAccountQuota;
  busy: boolean;
  onDelete: (account: Sub2ApiAccountQuota) => void;
}) {
  const tone =
    account.status === "ready"
      ? "ok"
      : account.status === "error"
        ? "danger"
        : "warn";
  const five = account.fiveHour?.remainingPercent;
  const seven = account.sevenDay?.remainingPercent;

  return (
    <article className={`account-mini tone-${tone}`}>
      <header className="account-mini-head">
        <div>
          <div className="account-mini-name">{account.name}</div>
          {account.email ? (
            <div className="account-mini-email mono">{account.email}</div>
          ) : null}
        </div>
        <span className={`pill status-${tone}`}>{statusLabel(account.status)}</span>
      </header>

      {account.errorMessage ? (
        <p className="account-mini-error">{account.errorMessage}</p>
      ) : null}

      {account.status === "ready" || five != null || seven != null ? (
        <div className="account-mini-meters">
          <ProgressBar
            value={five ?? 0}
            invertTone
            label={
              five == null
                ? "5h · 无数据"
                : `5h ${five.toFixed(0)}% · reset ${formatDuration(account.fiveHour?.resetAfterSeconds ?? 0)}`
            }
          />
          <ProgressBar
            value={seven ?? 0}
            invertTone
            label={
              seven == null
                ? "7d · 无数据"
                : `7d ${seven.toFixed(0)}% · reset ${formatDuration(account.sevenDay?.resetAfterSeconds ?? 0)}`
            }
          />
        </div>
      ) : (
        <p className="muted-line">此账号无 5h/7d 额度窗口（可能已失效）</p>
      )}

      <div className="account-mini-actions">
        <button
          type="button"
          className="btn ghost danger-text"
          disabled={busy}
          onClick={() => onDelete(account)}
        >
          删除
        </button>
      </div>
    </article>
  );
}

export function Sub2ApiCard() {
  const [data, setData] = useState<Sub2ApiUsage | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      setData(await api.getSub2apiUsage());
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
    const id = window.setInterval(() => void refresh(), REFRESH_MS.sub2api);
    return () => window.clearInterval(id);
  }, [refresh]);

  const onDelete = async (account: Sub2ApiAccountQuota) => {
    const label = account.email || account.name || `#${account.id}`;
    const ok = window.confirm(
      `确认从号池删除 OAuth 账号「${label}」？\n此操作不可恢复（不会删除 AIHub/AnyRouter）。`,
    );
    if (!ok) return;
    setBusy(true);
    setError(null);
    try {
      await api.deleteSub2apiAccount(account.id);
      setData(await api.getSub2apiUsage());
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const errored = data?.accounts.filter((a) => a.status === "error").length ?? 0;

  return (
    <CardShell
      title="Sub2API 号池"
      subtitle="仅 OpenAI/Codex OAuth 账号 · 中转站不计入"
      refreshedAt={data?.fetchedAt}
      onRefresh={() => void refresh()}
      refreshing={busy}
    >
      {error ? <p className="error-line">{error}</p> : null}

      <div className="metric-row">
        <div>
          <div className="metric-label">可用 OAuth</div>
          <div className="metric-value mono">
            {data ? `${data.poolAvailable}/${data.poolTotal}` : "—"}
          </div>
        </div>
        <div>
          <div className="metric-label">异常账号</div>
          <div className={`metric-value mono ${errored > 0 ? "danger-text" : ""}`}>
            {data ? errored : "—"}
          </div>
        </div>
      </div>

      {data && data.poolAvailable > 0 ? (
        <div className="account-mini-meters summary-meters">
          <ProgressBar
            value={data.fiveHour.remainingPercent}
            invertTone
            label={`可用号平均 5h ${data.fiveHour.remainingPercent.toFixed(0)}% · reset ${formatDuration(data.fiveHour.resetAfterSeconds)}`}
          />
          <ProgressBar
            value={data.sevenDay.remainingPercent}
            invertTone
            label={`可用号平均 7d ${data.sevenDay.remainingPercent.toFixed(0)}% · reset ${formatDuration(data.sevenDay.resetAfterSeconds)}`}
          />
        </div>
      ) : null}

      <div className="account-mini-list">
        {data?.accounts.length ? (
          data.accounts.map((a) => (
            <AccountMiniCard
              key={a.id}
              account={a}
              busy={busy}
              onDelete={(acc) => void onDelete(acc)}
            />
          ))
        ) : data ? (
          <p className="muted-line">没有 OAuth/GPT 号池账号（AIHub/AnyRouter 等中转站请看「供应商」卡）</p>
        ) : null}
      </div>
    </CardShell>
  );
}
