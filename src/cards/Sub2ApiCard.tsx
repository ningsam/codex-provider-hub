import { useCallback, useEffect, useState } from "react";
import { open as chooseFile } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import { CardShell } from "../components/CardShell";
import { ProgressBar } from "../components/ProgressBar";
import { api, REFRESH_MS } from "../lib/api";
import { formatDuration } from "../lib/format";
import type {
  Sub2ApiAccountQuota,
  Sub2ApiBrowserLoginStatus,
  Sub2ApiImportResult,
  Sub2ApiUsage,
} from "../types";

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
  const [importResult, setImportResult] = useState<Sub2ApiImportResult | null>(null);
  const [browserLogin, setBrowserLogin] = useState<Sub2ApiBrowserLoginStatus | null>(null);
  const [callbackUrl, setCallbackUrl] = useState("");

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

  const chooseImportFile = async (kind: "json" | "txt") => {
    const path = await chooseFile({
      multiple: false,
      directory: false,
      filters:
        kind === "json"
          ? [{ name: "Codex OAuth JSON", extensions: ["json", "jsonl"] }]
          : [{ name: "Card export TXT", extensions: ["txt"] }],
    });
    if (!path || Array.isArray(path)) return;
    const ok = window.confirm(
      `确认导入本地文件「${path.split("/").pop() ?? path}」？\n凭据仅会传给本机 Sub2API，不会显示在 Hub 中。`,
    );
    if (!ok) return;
    setBusy(true);
    setError(null);
    setImportResult(null);
    try {
      const result = await api.importSub2apiFile(path);
      setImportResult(result);
      setData(await api.getSub2apiUsage());
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const startBrowserLogin = async () => {
    setBusy(true);
    setError(null);
    setImportResult(null);
    try {
      const status = await api.beginSub2apiBrowserLogin();
      setBrowserLogin(status);
      setCallbackUrl("");
      await openUrl(status.loginUrl);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setBrowserLogin(null);
    } finally {
      setBusy(false);
    }
  };

  const completeBrowserLogin = async () => {
    const sessionId = browserLogin?.sessionId;
    if (!sessionId || !callbackUrl.trim()) return;
    setBusy(true);
    setError(null);
    try {
      const status = await api.completeSub2apiBrowserLogin(sessionId, callbackUrl.trim());
      setBrowserLogin(status);
      setData(await api.getSub2apiUsage());
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const cancelBrowserLogin = async () => {
    const sessionId = browserLogin?.sessionId;
    if (!sessionId) return;
    try {
      await api.cancelSub2apiBrowserLogin(sessionId);
      setBrowserLogin(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

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

      <section className="account-mini-list">
        <div className="account-mini-head">
          <div>
            <div className="account-mini-name">导入 OAuth 账号</div>
            <div className="muted-line">仅 OpenAI/Codex；凭据只在本机处理</div>
          </div>
        </div>
        <div className="account-mini-actions">
          <button type="button" className="btn ghost" disabled={busy} onClick={() => void chooseImportFile("json")}>
            导入 JSON
          </button>
          <button type="button" className="btn ghost" disabled={busy} onClick={() => void chooseImportFile("txt")}>
            导入 TXT
          </button>
          <button type="button" className="btn" disabled={busy || browserLogin?.state === "waiting"} onClick={() => void startBrowserLogin()}>
            浏览器登录 + 2FA
          </button>
        </div>
        {browserLogin ? (
          <div className={browserLogin.state === "complete" ? "muted-line" : "error-line"}>
            <p>
              {browserLogin.message}
              {browserLogin.importedAccounts.length ? ` ${browserLogin.importedAccounts.join("、")}` : ""}
            </p>
            {browserLogin.state === "waiting" ? (
              <div className="account-mini-actions">
                <input
                  className="field-input mono"
                  value={callbackUrl}
                  onChange={(event) => setCallbackUrl(event.target.value)}
                  placeholder="粘贴登录后最终跳转的完整 URL"
                  aria-label="OAuth callback URL"
                />
                <button type="button" className="btn" disabled={busy || !callbackUrl.trim()} onClick={() => void completeBrowserLogin()}>
                  完成导入
                </button>
              <button type="button" className="btn ghost danger-text" onClick={() => void cancelBrowserLogin()}>
                取消
              </button>
              </div>
            ) : null}
          </div>
        ) : null}
        {importResult ? (
          <p className="muted-line">
            {importResult.summary} 新增 {importResult.created} · 更新 {importResult.updated} · 跳过 {importResult.skipped} · 失败 {importResult.failed}
          </p>
        ) : null}
      </section>

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
