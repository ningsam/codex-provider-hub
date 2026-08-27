import { useCallback, useEffect, useState } from "react";
import { open as chooseFile } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import { CardShell } from "../components/CardShell";
import { ProgressBar } from "../components/ProgressBar";
import { useI18n } from "../i18n";
import { api, REFRESH_MS } from "../lib/api";
import { formatDuration } from "../lib/format";
import type {
  Sub2ApiAccountQuota,
  Sub2ApiBrowserLoginStatus,
  Sub2ApiImportResult,
  Sub2ApiUsage,
} from "../types";

function AccountMiniCard({
  account,
  busy,
  onDelete,
}: {
  account: Sub2ApiAccountQuota;
  busy: boolean;
  onDelete: (account: Sub2ApiAccountQuota) => void;
}) {
  const { t } = useI18n();
  const tone =
    account.status === "ready"
      ? "ok"
      : account.status === "error"
        ? "danger"
        : "warn";
  const statusText =
    account.status === "ready"
      ? t("sub2api.ready")
      : account.status === "error"
        ? t("sub2api.errorStatus")
        : account.status === "inactive"
          ? t("sub2api.inactive")
          : account.status || t("sub2api.unknown");
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
        <span className={`pill status-${tone}`}>{statusText}</span>
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
                ? `5h · ${t("sub2api.noData")}`
                : t("sub2api.account5h", {
                    percent: five.toFixed(0),
                    reset: formatDuration(account.fiveHour?.resetAfterSeconds ?? 0),
                  })
            }
          />
          <ProgressBar
            value={seven ?? 0}
            invertTone
            label={
              seven == null
                ? `7d · ${t("sub2api.noData")}`
                : t("sub2api.account7d", {
                    percent: seven.toFixed(0),
                    reset: formatDuration(account.sevenDay?.resetAfterSeconds ?? 0),
                  })
            }
          />
        </div>
      ) : (
        <p className="muted-line">{t("sub2api.noWindow")}</p>
      )}

      <div className="account-mini-actions">
        <button
          type="button"
          className="btn ghost danger-text"
          disabled={busy}
          onClick={() => onDelete(account)}
        >
          {t("common.delete")}
        </button>
      </div>
    </article>
  );
}

export function Sub2ApiCard() {
  const { locale, t } = useI18n();
  const [data, setData] = useState<Sub2ApiUsage | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [importResult, setImportResult] = useState<Sub2ApiImportResult | null>(null);
  const [browserLogin, setBrowserLogin] = useState<Sub2ApiBrowserLoginStatus | null>(null);

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

  useEffect(() => {
    const sessionId = browserLogin?.sessionId;
    if (!sessionId || browserLogin.state !== "waiting") return;
    const id = window.setInterval(() => {
      void api
        .getSub2apiBrowserLoginStatus(sessionId)
        .then((status) => setBrowserLogin(status))
        .catch((e) => {
          setError(e instanceof Error ? e.message : String(e));
          setBrowserLogin(null);
        });
    }, 1_500);
    return () => window.clearInterval(id);
  }, [browserLogin?.sessionId, browserLogin?.state]);

  useEffect(() => {
    const sessionId = browserLogin?.sessionId;
    if (!sessionId || browserLogin.state !== "ready") return;
    void (async () => {
      setBusy(true);
      try {
        const status = await api.completeSub2apiBrowserLogin(sessionId);
        setBrowserLogin(status);
        setData(await api.getSub2apiUsage());
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
        setBrowserLogin(null);
      } finally {
        setBusy(false);
      }
    })();
  }, [browserLogin?.sessionId, browserLogin?.state]);

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
    const fileName = path.split(/[\\/]/).pop() ?? path;
    const ok = window.confirm(t("sub2api.importConfirm", { name: fileName }));
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
      await openUrl(status.loginUrl);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setBrowserLogin(null);
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
    const ok = window.confirm(t("sub2api.deleteConfirm", { name: label }));
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

  const errored = data?.accounts.filter((account) => account.status === "error").length ?? 0;
  const accountSeparator = locale === "zh-CN" ? "、" : ", ";

  return (
    <CardShell
      title={t("sub2api.title")}
      subtitle={t("sub2api.subtitle")}
      refreshedAt={data?.fetchedAt}
      onRefresh={() => void refresh()}
      refreshing={busy}
    >
      {error ? <p className="error-line">{error}</p> : null}

      <section className="account-mini-list">
        <div className="account-mini-head">
          <div>
            <div className="account-mini-name">{t("sub2api.importTitle")}</div>
            <div className="muted-line">{t("sub2api.importPrivate")}</div>
          </div>
        </div>
        <div className="account-mini-actions">
          <button
            type="button"
            className="btn ghost"
            disabled={busy}
            onClick={() => void chooseImportFile("json")}
          >
            {t("sub2api.importJson")}
          </button>
          <button
            type="button"
            className="btn ghost"
            disabled={busy}
            onClick={() => void chooseImportFile("txt")}
          >
            {t("sub2api.importTxt")}
          </button>
          <button
            type="button"
            className="btn"
            disabled={busy || browserLogin?.state === "waiting"}
            onClick={() => void startBrowserLogin()}
          >
            {t("sub2api.browserLogin")}
          </button>
        </div>
        {browserLogin ? (
          <div className={browserLogin.state === "complete" ? "muted-line" : "error-line"}>
            <p>
              {browserLogin.message}
              {browserLogin.importedAccounts.length
                ? ` ${browserLogin.importedAccounts.join(accountSeparator)}`
                : ""}
            </p>
            {browserLogin.state === "waiting" ? (
              <div className="account-mini-actions">
                <button
                  type="button"
                  className="btn ghost danger-text"
                  onClick={() => void cancelBrowserLogin()}
                >
                  {t("common.cancel")}
                </button>
              </div>
            ) : null}
          </div>
        ) : null}
        {importResult ? (
          <p className="muted-line">
            {t("sub2api.importSummary", {
              summary: importResult.summary,
              created: importResult.created,
              updated: importResult.updated,
              skipped: importResult.skipped,
              failed: importResult.failed,
            })}
          </p>
        ) : null}
      </section>

      <div className="metric-row">
        <div>
          <div className="metric-label">{t("sub2api.available")}</div>
          <div className="metric-value mono">
            {data ? `${data.poolAvailable}/${data.poolTotal}` : "—"}
          </div>
        </div>
        <div>
          <div className="metric-label">{t("sub2api.errors")}</div>
          <div className={`metric-value mono ${errored > 0 ? "danger-text" : ""}`}>
            {data ? errored : "—"}
          </div>
        </div>
      </div>

      {data && data.poolAvailable > 0 && (data.fiveHour || data.sevenDay) ? (
        <div className="account-mini-meters summary-meters">
          {data.fiveHour ? (
            <ProgressBar
              value={data.fiveHour.remainingPercent}
              invertTone
              label={t("sub2api.average5h", {
                percent: data.fiveHour.remainingPercent.toFixed(0),
                reset: formatDuration(data.fiveHour.resetAfterSeconds),
              })}
            />
          ) : null}
          {data.sevenDay ? (
            <ProgressBar
              value={data.sevenDay.remainingPercent}
              invertTone
              label={t("sub2api.average7d", {
                percent: data.sevenDay.remainingPercent.toFixed(0),
                reset: formatDuration(data.sevenDay.resetAfterSeconds),
              })}
            />
          ) : null}
        </div>
      ) : null}

      <div className="account-mini-list">
        {data?.accounts.length ? (
          data.accounts.map((account) => (
            <AccountMiniCard
              key={account.id}
              account={account}
              busy={busy}
              onDelete={(item) => void onDelete(item)}
            />
          ))
        ) : data ? (
          <p className="muted-line">{t("sub2api.noAccounts")}</p>
        ) : null}
      </div>
    </CardShell>
  );
}
