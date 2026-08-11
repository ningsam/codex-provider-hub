import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type FormEvent,
} from "react";
import { open as chooseFile } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import { CardShell } from "../components/CardShell";
import { ProgressBar } from "../components/ProgressBar";
import { api, REFRESH_MS } from "../lib/api";
import { formatDuration } from "../lib/format";
import type {
  OfficialQuotaProbe,
  Sub2ApiAccountQuota,
  Sub2ApiBrowserLoginStatus,
  Sub2ApiImportResult,
  Sub2ApiRoutingPolicy,
  Sub2ApiUsage,
} from "../types";

type Tone = "ok" | "warn" | "danger";
type AccountActionKind = "select" | "recover" | "delete";

interface AccountAction {
  accountId: number;
  kind: AccountActionKind;
}

interface UiError {
  source: "refresh" | "operation";
  message: string;
}

interface BrowserLoginError {
  sessionId: string;
  message: string;
}

const ROUTING_POLICIES: readonly Sub2ApiRoutingPolicy[] = [
  "oauthFirst",
  "relayFirst",
  "balanced",
];

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function statusLabel(status: string): string {
  switch (status) {
    case "ready":
    case "available":
      return "可用";
    case "parked":
    case "rate_limited":
    case "model_rate_limited":
    case "temporary":
      return "已停车";
    case "quota_exhausted":
      return "额度用尽";
    case "overload":
    case "overloaded":
      return "上游繁忙";
    case "expired":
      return "凭据过期";
    case "error":
    case "banned":
      return "异常/封禁";
    case "inactive":
    case "paused":
      return "停用";
    default:
      return status || "未知";
  }
}

function routingStateLabel(state: string): string {
  switch (state) {
    case "automatic":
      return "自动调度";
    case "preferred":
      return "当前首选";
    case "failover":
      return "故障切换";
    case "unavailable":
      return "无可用账号";
    case "fallback_missing":
      return "兜底缺失";
    case "unconfigured":
      return "未配置";
    case "stale":
      return "状态待刷新";
    case "error":
      return "状态异常";
    default:
      return state || "状态未知";
  }
}

function routingTone(state: string): Tone {
  if (state === "unavailable" || state === "error") return "danger";
  if (
    state === "failover" ||
    state === "fallback_missing" ||
    state === "unconfigured" ||
    state === "stale"
  ) {
    return "warn";
  }
  return "ok";
}

function policyLabel(policy: string): string {
  switch (policy) {
    case "oauthFirst":
      return "OAuth优先";
    case "relayFirst":
      return "中转优先";
    case "balanced":
      return "均衡";
    default:
      return policy || "未知策略";
  }
}

function accountTypeLabel(accountType: string | null): string {
  const normalized = accountType?.toLowerCase();
  if (normalized === "oauth") return "OAuth";
  if (normalized === "apikey" || normalized === "api_key" || normalized === "relay") {
    return "API Key";
  }
  return accountType || "未知类型";
}

function isRelayAccountType(accountType: string | null | undefined): boolean {
  const normalized = accountType?.toLowerCase();
  return normalized === "apikey" || normalized === "api_key" || normalized === "relay";
}

function formatUnavailableUntil(value: string | null): string | null {
  if (!value) return null;
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return null;
  return date.toLocaleString("zh-CN", {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  });
}

function displayName(account: Sub2ApiAccountQuota): string {
  return account.email || account.name || `#${account.id}`;
}

function AccountMiniCard({
  account,
  pendingAction,
  disabled,
  onSelect,
  onRecover,
  onDelete,
  officialProbe,
  officialError,
  officialBusy,
  onProbeOfficial,
}: {
  account: Sub2ApiAccountQuota;
  pendingAction: AccountAction | null;
  disabled: boolean;
  onSelect: (account: Sub2ApiAccountQuota) => void;
  onRecover: (account: Sub2ApiAccountQuota) => void;
  onDelete: (account: Sub2ApiAccountQuota) => void;
  officialProbe: OfficialQuotaProbe | null;
  officialError: string | null;
  officialBusy: boolean;
  onProbeOfficial: (account: Sub2ApiAccountQuota) => void;
}) {
  const officialUnavailable =
    officialProbe != null && (!officialProbe.allowed || officialProbe.limitReached);
  const officialUnavailableReason = !officialProbe
    ? null
    : !officialProbe.allowed && officialProbe.limitReached
      ? "官方实测：当前账号不允许请求，且额度已达上限"
      : !officialProbe.allowed
        ? "官方实测：OpenAI 当前不允许该账号发起请求"
        : officialProbe.limitReached
          ? "官方实测：额度已达上限"
          : null;
  const tone: Tone =
    officialUnavailable
      ? "danger"
      : account.availability === "model_rate_limited"
      ? "warn"
      : account.available
        ? "ok"
        : account.recoverable
          ? "warn"
          : "danger";
  const five = account.fiveHour?.remainingPercent;
  const seven = account.sevenDay?.remainingPercent;
  const pending = pendingAction?.accountId === account.id ? pendingAction.kind : null;
  const availabilityReason =
    account.availabilityReason.trim() || statusLabel(account.availability || account.status);
  const unavailableUntil = formatUnavailableUntil(account.unavailableUntil);
  const showError =
    account.errorMessage.trim() &&
    account.errorMessage.trim() !== account.availabilityReason.trim();

  return (
    <article
      className={`account-mini tone-${tone}`}
      aria-busy={pending != null}
    >
      <header className="account-mini-head">
        <div className="account-mini-copy">
          <div className="account-mini-name">{account.name}</div>
          {account.email ? (
            <div className="account-mini-email mono">{account.email}</div>
          ) : null}
        </div>
        <div className="account-mini-badges">
          {account.preferred ? (
            <span className="pill status-current">当前首选</span>
          ) : null}
          <span className={`pill status-${tone}`}>
            {officialUnavailable
              ? "官方不可用"
              : statusLabel(account.availability || account.status)}
          </span>
        </div>
      </header>

      <p className={`account-mini-availability status-${tone}`}>
        <span>可用性</span>
        {officialUnavailableReason || availabilityReason}
        {!officialUnavailableReason && unavailableUntil ? ` · 至 ${unavailableUntil}` : ""}
      </p>

      {showError ? (
        <p className="account-mini-error">{account.errorMessage}</p>
      ) : null}

      {account.available || five != null || seven != null ? (
        <div className="account-mini-meters">
          <ProgressBar
            value={five ?? 0}
            invertTone
            label={
              five == null
                ? "本地 5h · 无数据"
                : `本地 5h 剩余 ${five.toFixed(0)}% · reset ${formatDuration(account.fiveHour?.resetAfterSeconds ?? 0)}`
            }
          />
          <ProgressBar
            value={seven ?? 0}
            invertTone
            label={
              seven == null
                ? "本地 7d · 无数据"
                : `本地 7d 剩余 ${seven.toFixed(0)}% · reset ${formatDuration(account.sevenDay?.resetAfterSeconds ?? 0)}`
            }
          />
        </div>
      ) : (
        <p className="muted-line">此账号无 5h/7d 额度窗口（可能已失效）</p>
      )}

      {officialProbe ? (
        <div
          className={`official-quota ${officialUnavailable ? "is-exhausted" : ""}`}
          role="status"
        >
          <div className="official-quota-head">
            <strong>官方实测 · {officialProbe.planType}</strong>
            <span>{officialUnavailable ? "官方不可用" : "官方可用"}</span>
          </div>
          {officialUnavailableReason ? (
            <p className="account-mini-error">{officialUnavailableReason}</p>
          ) : null}
          <div className="account-mini-meters">
            {officialProbe.fiveHour ? (
              <ProgressBar
                value={Math.max(0, 100 - officialProbe.fiveHour.usedPercent)}
                invertTone
                label={`官方 5h 已用 ${officialProbe.fiveHour.usedPercent.toFixed(0)}%${officialProbe.fiveHour.limitReached ? " · LIMIT REACHED" : ""} · reset ${formatDuration(officialProbe.fiveHour.resetAfterSeconds)}`}
              />
            ) : null}
            {officialProbe.sevenDay ? (
              <ProgressBar
                value={Math.max(0, 100 - officialProbe.sevenDay.usedPercent)}
                invertTone
                label={`官方 7d 已用 ${officialProbe.sevenDay.usedPercent.toFixed(0)}%${officialProbe.sevenDay.limitReached ? " · LIMIT REACHED" : ""} · reset ${formatDuration(officialProbe.sevenDay.resetAfterSeconds)}`}
              />
            ) : null}
          </div>
        </div>
      ) : null}

      {officialError ? <p className="account-mini-error">官方探测：{officialError}</p> : null}

      <div className="account-mini-actions">
        <button
          type="button"
          className="btn ghost"
          disabled={disabled || officialBusy}
          aria-label={`实测 ${displayName(account)} 的 OpenAI 官方额度`}
          onClick={() => onProbeOfficial(account)}
        >
          {officialBusy ? "探测中…" : officialProbe ? "重新实测" : "官方实测"}
        </button>
        {!account.preferred ? (
          <button
            type="button"
            className="btn primary"
            disabled={disabled || !account.available || officialUnavailable}
            title={
              officialUnavailableReason ||
              (!account.available ? availabilityReason : undefined)
            }
            aria-label={`将 ${displayName(account)} 设为当前首选账号`}
            onClick={() => onSelect(account)}
          >
            {pending === "select" ? "切换中…" : "设为当前"}
          </button>
        ) : null}
        {account.recoverable ? (
          <button
            type="button"
            className="btn ghost"
            disabled={disabled}
            aria-label={`立即恢复 ${displayName(account)}`}
            onClick={() => onRecover(account)}
          >
            {pending === "recover" ? "恢复中…" : "立即恢复"}
          </button>
        ) : null}
        <button
          type="button"
          className="btn ghost danger-text"
          disabled={disabled}
          aria-label={`删除 ${displayName(account)}`}
          onClick={() => onDelete(account)}
        >
          {pending === "delete" ? "删除中…" : "删除"}
        </button>
      </div>
    </article>
  );
}

export function Sub2ApiCard() {
  const [data, setData] = useState<Sub2ApiUsage | null>(null);
  const [error, setError] = useState<UiError | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [importBusy, setImportBusy] = useState(false);
  const [accountAction, setAccountAction] = useState<AccountAction | null>(null);
  const [thresholdBusy, setThresholdBusy] = useState(false);
  const [pendingPolicy, setPendingPolicy] = useState<Sub2ApiRoutingPolicy | null>(null);
  const [thresholdDraft, setThresholdDraft] = useState("");
  const [thresholdDirty, setThresholdDirty] = useState(false);
  const [importResult, setImportResult] = useState<Sub2ApiImportResult | null>(null);
  const [browserLogin, setBrowserLogin] = useState<Sub2ApiBrowserLoginStatus | null>(null);
  const [browserLoginError, setBrowserLoginError] = useState<BrowserLoginError | null>(null);
  const [officialProbes, setOfficialProbes] = useState<Record<number, OfficialQuotaProbe>>({});
  const [officialErrors, setOfficialErrors] = useState<Record<number, string>>({});
  const [officialBusyAccountId, setOfficialBusyAccountId] = useState<number | null>(null);

  const usageRequestId = useRef(0);
  const usageOperation = useRef(false);
  const refreshInFlight = useRef(false);
  const foregroundRefresh = useRef(false);
  const completingBrowserSession = useRef<string | null>(null);
  const pollingBrowserSession = useRef<string | null>(null);
  const automaticallyCompletedBrowserSession = useRef<string | null>(null);

  const refresh = useCallback(async (silent = false) => {
    if (usageOperation.current || refreshInFlight.current) return;

    const requestId = ++usageRequestId.current;
    refreshInFlight.current = true;
    if (!silent) {
      foregroundRefresh.current = true;
      setRefreshing(true);
      setError(null);
      setNotice(null);
    }

    try {
      const next = await api.getSub2apiUsage();
      if (requestId !== usageRequestId.current) return;
      setData(next);
      setError((current) => (current?.source === "refresh" ? null : current));
    } catch (e) {
      if (requestId !== usageRequestId.current) return;
      const message = errorMessage(e);
      setError((current) =>
        silent && current?.source === "operation"
          ? current
          : { source: "refresh", message },
      );
    } finally {
      refreshInFlight.current = false;
      if (!silent) {
        foregroundRefresh.current = false;
        setRefreshing(false);
      }
    }
  }, []);

  useEffect(() => {
    void refresh();
    const id = window.setInterval(() => void refresh(true), REFRESH_MS.sub2api);
    return () => window.clearInterval(id);
  }, [refresh]);

  useEffect(() => {
    if (!data || thresholdDirty || thresholdBusy) return;
    setThresholdDraft(String(data.routing.autoPauseThresholdPercent));
  }, [data, thresholdBusy, thresholdDirty]);

  const checkBrowserLoginStatus = useCallback(async (sessionId: string) => {
    if (pollingBrowserSession.current === sessionId) return;
    pollingBrowserSession.current = sessionId;
    try {
      const status = await api.getSub2apiBrowserLoginStatus(sessionId);
      setBrowserLogin((current) =>
        current?.sessionId === sessionId ? status : current,
      );
      setBrowserLoginError((current) =>
        current?.sessionId === sessionId ? null : current,
      );
    } catch (e) {
      setBrowserLoginError({
        sessionId,
        message: `检查浏览器登录状态失败：${errorMessage(e)}。会话仍保留，Hub 会继续自动检查。`,
      });
    } finally {
      if (pollingBrowserSession.current === sessionId) {
        pollingBrowserSession.current = null;
      }
    }
  }, []);

  useEffect(() => {
    const sessionId = browserLogin?.sessionId;
    if (!sessionId || browserLogin.state !== "waiting") return;
    const id = window.setInterval(() => {
      void checkBrowserLoginStatus(sessionId);
    }, 1_500);
    return () => window.clearInterval(id);
  }, [browserLogin?.sessionId, browserLogin?.state, checkBrowserLoginStatus]);

  const completeBrowserLogin = useCallback(async (sessionId: string) => {
    if (
      usageOperation.current ||
      foregroundRefresh.current ||
      completingBrowserSession.current === sessionId
    ) {
      return;
    }

    completingBrowserSession.current = sessionId;
    usageOperation.current = true;
    const requestId = ++usageRequestId.current;
    setImportBusy(true);
    setError(null);
    setNotice(null);
    setBrowserLoginError((current) =>
      current?.sessionId === sessionId ? null : current,
    );

    let completedStatus: Sub2ApiBrowserLoginStatus;
    try {
      completedStatus = await api.completeSub2apiBrowserLogin(sessionId);
    } catch (e) {
      if (requestId === usageRequestId.current) {
        const message = errorMessage(e);
        setBrowserLogin((current) =>
          current?.sessionId === sessionId
            ? {
                ...current,
                state: "ready",
                message: "已收到浏览器回调，但账号导入尚未完成。",
              }
            : current,
        );
        setBrowserLoginError({
          sessionId,
          message: `账号导入未完成：${message}。OAuth 会话已保留，可直接重试导入。`,
        });
      }
      usageOperation.current = false;
      completingBrowserSession.current = null;
      setImportBusy(false);
      return;
    }

    if (requestId === usageRequestId.current) {
      setBrowserLogin(completedStatus);
      setNotice("浏览器登录账号已加入号池。");
    }

    try {
      const next = await api.getSub2apiUsage();
      if (requestId === usageRequestId.current) {
        setData(next);
      }
    } catch (e) {
      if (requestId === usageRequestId.current) {
        setError({
          source: "refresh",
          message: `账号已导入，但号池刷新失败：${errorMessage(e)}。可点击刷新重试。`,
        });
      }
    } finally {
      usageOperation.current = false;
      completingBrowserSession.current = null;
      setImportBusy(false);
    }
  }, []);

  useEffect(() => {
    const sessionId = browserLogin?.sessionId;
    if (!sessionId || browserLogin.state !== "ready") return;
    if (automaticallyCompletedBrowserSession.current === sessionId) return;
    automaticallyCompletedBrowserSession.current = sessionId;
    void completeBrowserLogin(sessionId);
  }, [browserLogin?.sessionId, browserLogin?.state, completeBrowserLogin]);

  const chooseImportFile = async (kind: "json" | "txt") => {
    setError(null);
    setNotice(null);
    setImportResult(null);
    let path: string | string[] | null;
    try {
      path = await chooseFile({
        multiple: false,
        directory: false,
        filters:
          kind === "json"
            ? [{ name: "Codex OAuth JSON", extensions: ["json", "jsonl"] }]
            : [{ name: "Card export TXT", extensions: ["txt"] }],
      });
    } catch (e) {
      setError({ source: "operation", message: errorMessage(e) });
      return;
    }
    if (!path || Array.isArray(path)) return;
    const ok = window.confirm(
      `确认导入本地文件「${path.split("/").pop() ?? path}」？\n凭据仅会传给本机 Sub2API，不会显示在 Hub 中。`,
    );
    if (!ok || usageOperation.current || foregroundRefresh.current) return;

    usageOperation.current = true;
    const requestId = ++usageRequestId.current;
    setImportBusy(true);
    setError(null);
    setImportResult(null);
    try {
      const result = await api.importSub2apiFile(path);
      const next = await api.getSub2apiUsage();
      if (requestId === usageRequestId.current) {
        setImportResult(result);
        setData(next);
      }
    } catch (e) {
      if (requestId === usageRequestId.current) {
        setError({ source: "operation", message: errorMessage(e) });
      }
    } finally {
      usageOperation.current = false;
      setImportBusy(false);
    }
  };

  const startBrowserLogin = async () => {
    if (usageOperation.current || foregroundRefresh.current) return;
    usageOperation.current = true;
    setImportBusy(true);
    setError(null);
    setNotice(null);
    setImportResult(null);
    setBrowserLoginError(null);
    try {
      const status = await api.beginSub2apiBrowserLogin();
      setBrowserLogin(status);
      await openUrl(status.loginUrl);
    } catch (e) {
      setError({ source: "operation", message: errorMessage(e) });
      setBrowserLogin(null);
    } finally {
      usageOperation.current = false;
      setImportBusy(false);
    }
  };

  const cancelBrowserLogin = async () => {
    const sessionId = browserLogin?.sessionId;
    if (!sessionId || usageOperation.current || foregroundRefresh.current) return;
    usageOperation.current = true;
    setImportBusy(true);
    setError(null);
    try {
      await api.cancelSub2apiBrowserLogin(sessionId);
      setBrowserLogin(null);
      setBrowserLoginError(null);
    } catch (e) {
      setError({ source: "operation", message: errorMessage(e) });
    } finally {
      usageOperation.current = false;
      setImportBusy(false);
    }
  };

  const runAccountMutation = async (
    account: Sub2ApiAccountQuota,
    kind: AccountActionKind,
    mutation: () => Promise<Sub2ApiUsage>,
    successMessage: string,
  ) => {
    if (usageOperation.current || foregroundRefresh.current) return;
    usageOperation.current = true;
    const requestId = ++usageRequestId.current;
    setAccountAction({ accountId: account.id, kind });
    setError(null);
    setNotice(null);
    try {
      const next = await mutation();
      if (requestId === usageRequestId.current) {
        setData(next);
        setNotice(successMessage);
      }
    } catch (e) {
      if (requestId === usageRequestId.current) {
        setError({ source: "operation", message: errorMessage(e) });
      }
    } finally {
      usageOperation.current = false;
      setAccountAction(null);
    }
  };

  const onSelect = async (account: Sub2ApiAccountQuota) => {
    await runAccountMutation(
      account,
      "select",
      () => api.setSub2apiCurrentAccount(account.id),
      `已将 ${displayName(account)} 设为当前首选。`,
    );
  };

  const onRecover = async (account: Sub2ApiAccountQuota) => {
    await runAccountMutation(
      account,
      "recover",
      () => api.recoverSub2apiAccount(account.id),
      `已恢复 ${displayName(account)} 的调度状态。`,
    );
  };

  const onDelete = async (account: Sub2ApiAccountQuota) => {
    const label = displayName(account);
    const ok = window.confirm(
      `确认从号池删除 OAuth 账号「${label}」？\n此操作不可恢复（不会删除 AIHub/AnyRouter）。`,
    );
    if (!ok) return;
    await runAccountMutation(
      account,
      "delete",
      async () => {
        await api.deleteSub2apiAccount(account.id);
        return api.getSub2apiUsage();
      },
      `已删除 ${label}。`,
    );
  };

  const onApplyThreshold = async (event: FormEvent) => {
    event.preventDefault();
    const threshold = Number(thresholdDraft);
    if (!Number.isInteger(threshold) || threshold < 1 || threshold > 100) {
      setError({ source: "operation", message: "自动摘除阈值必须是 1–100 的整数。" });
      return;
    }
    if (usageOperation.current || foregroundRefresh.current) return;

    usageOperation.current = true;
    const requestId = ++usageRequestId.current;
    setThresholdBusy(true);
    setError(null);
    setNotice(null);
    try {
      const next = await api.setSub2apiAutoPauseThreshold(threshold);
      if (requestId === usageRequestId.current) {
        setData(next);
        setThresholdDraft(String(next.routing.autoPauseThresholdPercent));
        setThresholdDirty(false);
        setNotice(
          threshold === 100
            ? "主动额度摘除已关闭。"
            : `自动摘除阈值已设为 ${threshold}%。`,
        );
      }
    } catch (e) {
      if (requestId === usageRequestId.current) {
        setError({ source: "operation", message: errorMessage(e) });
      }
    } finally {
      usageOperation.current = false;
      setThresholdBusy(false);
    }
  };

  const onChangePolicy = async (policy: Sub2ApiRoutingPolicy) => {
    if (
      (policy === data?.routing.policy && data.routing.policyConfigured) ||
      usageOperation.current ||
      foregroundRefresh.current
    ) {
      return;
    }

    usageOperation.current = true;
    const requestId = ++usageRequestId.current;
    setPendingPolicy(policy);
    setError(null);
    setNotice(null);
    try {
      const next = await api.setSub2apiRoutingPolicy(policy);
      if (requestId === usageRequestId.current) {
        setData(next);
        setNotice(`路由策略已切换为${policyLabel(next.routing.policy)}。`);
      }
    } catch (e) {
      if (requestId === usageRequestId.current) {
        setError({ source: "operation", message: errorMessage(e) });
      }
    } finally {
      usageOperation.current = false;
      setPendingPolicy(null);
    }
  };

  const onProbeOfficial = async (account: Sub2ApiAccountQuota) => {
    if (officialBusyAccountId != null) return;
    setOfficialBusyAccountId(account.id);
    setOfficialErrors((current) => {
      const next = { ...current };
      delete next[account.id];
      return next;
    });
    try {
      const probe = await api.probeSub2apiOfficialQuota(account.id);
      setOfficialProbes((current) => ({ ...current, [account.id]: probe }));
    } catch (e) {
      setOfficialErrors((current) => ({
        ...current,
        [account.id]: errorMessage(e),
      }));
    } finally {
      setOfficialBusyAccountId(null);
    }
  };

  const errored = data?.accounts.filter((account) => account.status === "error").length ?? 0;
  const interactionBusy =
    importBusy || accountAction != null || thresholdBusy || pendingPolicy != null;
  const controlsDisabled = refreshing || interactionBusy;
  const threshold = Number(thresholdDraft);
  const thresholdValid =
    Number.isInteger(threshold) && threshold >= 1 && threshold <= 100;
  const thresholdChanged =
    data != null && threshold !== data.routing.autoPauseThresholdPercent;
  const distribution = data?.routing.distribution ?? [];
  const lastSuccessfulAtMs = data?.routing.lastSuccessfulAt
    ? Date.parse(data.routing.lastSuccessfulAt)
    : Number.NaN;
  const lastSuccessfulAgeMs = Date.now() - lastSuccessfulAtMs;
  const relayFallbackObserved =
    data != null &&
    data.routing.oauthAvailableCount === 0 &&
    isRelayAccountType(data.routing.lastSuccessfulAccountType) &&
    data.routing.activeRelayName != null;
  const relayCarryingTraffic =
    relayFallbackObserved &&
    data.routing.relayAvailableCount > 0 &&
    Number.isFinite(lastSuccessfulAgeMs) &&
    lastSuccessfulAgeMs >= -60_000 &&
    lastSuccessfulAgeMs <= 120_000;
  const policyDeviation = data?.routing.policyDeviation ?? false;
  const policyUnconfigured = data != null && !data.routing.policyConfigured;
  const routeTone = policyUnconfigured
    ? "warn"
    : policyDeviation
    ? "danger"
    : relayCarryingTraffic
    ? "ok"
    : data
      ? routingTone(data.routing.state)
      : "ok";
  const routeStateText = policyUnconfigured
    ? "策略未应用"
    : policyDeviation
    ? "策略偏离"
    : relayCarryingTraffic
    ? "中转承接"
    : data
      ? routingStateLabel(data.routing.state)
      : "";
  const browserMessageClass =
    browserLogin?.state === "waiting" ||
    browserLogin?.state === "ready" ||
    browserLogin?.state === "complete"
      ? "hint-line"
      : "error-line";

  return (
    <CardShell
      title="Sub2API 号池"
      subtitle="仅 OpenAI/Codex OAuth 账号 · 中转站不计入"
      refreshedAt={data?.fetchedAt}
      onRefresh={() => void refresh()}
      refreshing={refreshing || interactionBusy}
    >
      {error ? (
        <p className="error-line" role="alert">
          {error.message}
        </p>
      ) : null}
      {notice ? (
        <p className="hint-line" role="status" aria-live="polite">
          {notice}
        </p>
      ) : null}
      {!data && refreshing ? (
        <p className="muted-line" role="status">
          正在读取号池…
        </p>
      ) : null}

      <section className="account-mini-list" aria-busy={importBusy}>
        <div className="account-mini-head">
          <div>
            <div className="account-mini-name">导入 OAuth 账号</div>
            <div className="muted-line">仅 OpenAI/Codex；凭据只在本机处理</div>
          </div>
        </div>
        <div className="account-mini-actions">
          <button
            type="button"
            className="btn ghost"
            disabled={controlsDisabled}
            onClick={() => void chooseImportFile("json")}
          >
            导入 JSON
          </button>
          <button
            type="button"
            className="btn ghost"
            disabled={controlsDisabled}
            onClick={() => void chooseImportFile("txt")}
          >
            导入 TXT
          </button>
          <button
            type="button"
            className="btn"
            disabled={
              controlsDisabled ||
              browserLogin?.state === "waiting" ||
              browserLogin?.state === "ready"
            }
            onClick={() => void startBrowserLogin()}
          >
            {importBusy && browserLogin?.state !== "waiting"
              ? "处理中…"
              : "浏览器登录 + 2FA"}
          </button>
        </div>
        {browserLogin ? (
          <div className={browserMessageClass} role="status" aria-live="polite">
            <p>
              {browserLogin.message}
              {browserLogin.importedAccounts.length
                ? ` ${browserLogin.importedAccounts.join("、")}`
                : ""}
            </p>
            {browserLoginError?.sessionId === browserLogin.sessionId ? (
              <p className="error-line" role="alert">
                {browserLoginError.message}
              </p>
            ) : null}
            {browserLogin.state === "waiting" ? (
              <div className="account-mini-actions">
                {browserLoginError?.sessionId === browserLogin.sessionId ? (
                  <button
                    type="button"
                    className="btn ghost"
                    disabled={controlsDisabled}
                    onClick={() => void checkBrowserLoginStatus(browserLogin.sessionId!)}
                  >
                    立即重试检查
                  </button>
                ) : null}
                <button
                  type="button"
                  className="btn ghost danger-text"
                  disabled={controlsDisabled}
                  onClick={() => void cancelBrowserLogin()}
                >
                  {importBusy ? "取消中…" : "取消"}
                </button>
              </div>
            ) : null}
            {browserLogin.state === "ready" ? (
              <div className="account-mini-actions">
                <button
                  type="button"
                  className="btn"
                  disabled={controlsDisabled}
                  onClick={() => void completeBrowserLogin(browserLogin.sessionId!)}
                >
                  {importBusy
                    ? "导入中…"
                    : browserLoginError?.sessionId === browserLogin.sessionId
                      ? "重试导入"
                      : "完成导入"}
                </button>
              </div>
            ) : null}
          </div>
        ) : null}
        {importResult ? (
          <p className="muted-line">
            {importResult.summary} 新增 {importResult.created} · 更新 {importResult.updated} · 跳过{" "}
            {importResult.skipped} · 失败 {importResult.failed}
          </p>
        ) : null}
      </section>

      {data ? (
        <section
          className={`pool-routing routing-${routeTone}`}
        >
          <div className="pool-routing-head">
            <div
              className="pool-routing-copy"
              role="status"
              aria-live="polite"
              aria-atomic="true"
            >
              <div className="pool-routing-title">
                <span>当前路由</span>
                <span className={`pill status-${routeTone}`}>
                  {routeStateText}
                </span>
              </div>
              <p>
                {data.routing.policyDeviationMessage ||
                  data.routing.message ||
                  routeStateText}
              </p>
            </div>
            <div className="route-window mono">
              <span>
                最近{data.routing.recentWindowMinutes}分钟 · 最多
                {data.routing.recentRequestLimit}条样本
              </span>
              <strong>{data.routing.recentRequestCount} 请求</strong>
            </div>
          </div>

          <div className="route-observation">
            <div
              className="route-last-success"
              title={
                data.routing.lastSuccessfulAccountId != null
                  ? `账号 #${data.routing.lastSuccessfulAccountId}`
                  : undefined
              }
            >
              <span>最近成功账号</span>
              <strong>
                {data.routing.lastSuccessfulAccountName ||
                  (data.routing.lastSuccessfulAccountId != null
                    ? `账号 #${data.routing.lastSuccessfulAccountId}`
                    : "暂无成功请求")}
              </strong>
              {data.routing.lastSuccessfulAccountType ? (
                <small>{accountTypeLabel(data.routing.lastSuccessfulAccountType)}</small>
              ) : null}
            </div>

            <div className="route-distribution" aria-label="最近请求账号分布">
              {distribution.length ? (
                distribution.map((entry) => {
                  const percent = Number.isFinite(entry.percent)
                    ? Math.max(0, Math.min(100, entry.percent))
                    : 0;
                  return (
                    <div className="route-distribution-row" key={`${entry.accountType}-${entry.accountId}`}>
                      <div className="route-distribution-meta">
                        <span title={`账号 #${entry.accountId}`}>{entry.name}</span>
                        <small>
                          {accountTypeLabel(entry.accountType)} · {entry.requestCount} 请求
                        </small>
                      </div>
                      <div
                        className="route-distribution-track"
                        role="progressbar"
                        aria-label={`${entry.name} 请求占比`}
                        aria-valuemin={0}
                        aria-valuemax={100}
                        aria-valuenow={percent}
                      >
                        <span style={{ width: `${percent}%` }} />
                      </div>
                      <span className="route-distribution-percent mono">
                        {percent.toFixed(0)}%
                      </span>
                    </div>
                  );
                })
              ) : (
                <p className="muted-line">最近窗口暂无请求</p>
              )}
            </div>
          </div>

          {relayCarryingTraffic ? (
            <p className="route-relay-hint" role="status">
              近 2 分钟最新成功请求由 {data.routing.activeRelayName}{' '}
              承接；生产组 OAuth 当前无可用账号。
            </p>
          ) : relayFallbackObserved ? (
            <p className="route-relay-hint" role="status">
              最近一次成功请求由 {data.routing.activeRelayName}
              承接；生产组 OAuth 当前无可用账号。该样本不等同于当前实时路由。
            </p>
          ) : null}

          {policyDeviation && data.routing.policyDeviationMessage ? (
            <p className="route-policy-deviation" role="alert">
              {data.routing.policyDeviationMessage}
            </p>
          ) : null}

          <div className="pool-routing-controls">
            <div className="routing-policy-control">
              <span id="sub2api-routing-policy-label">路由策略</span>
              <div
                className="segmented-control"
                role="group"
                aria-labelledby="sub2api-routing-policy-label"
                aria-busy={pendingPolicy != null}
              >
                {ROUTING_POLICIES.map((policy) => {
                  const active =
                    data.routing.policyConfigured && data.routing.policy === policy;
                  return (
                    <button
                      key={policy}
                      type="button"
                      className={active ? "is-active" : ""}
                      aria-pressed={active}
                      disabled={controlsDisabled || active}
                      onClick={() => void onChangePolicy(policy)}
                    >
                      {pendingPolicy === policy ? "应用中…" : policyLabel(policy)}
                    </button>
                  );
                })}
              </div>
            </div>

            <form
              className="auto-pause-form"
              onSubmit={(event) => void onApplyThreshold(event)}
            >
              <label htmlFor="sub2api-auto-pause-threshold">自动摘除阈值</label>
              <div className="auto-pause-controls">
                <input
                  id="sub2api-auto-pause-threshold"
                  type="number"
                  min={1}
                  max={100}
                  step={1}
                  inputMode="numeric"
                  value={thresholdDraft}
                  disabled={controlsDisabled}
                  aria-invalid={thresholdDraft !== "" && !thresholdValid}
                  title="100% 表示关闭主动额度摘除"
                  onChange={(event) => {
                    setThresholdDraft(event.target.value);
                    setThresholdDirty(true);
                  }}
                />
                <span aria-hidden="true">%</span>
                <button
                  type="submit"
                  className="btn ghost"
                  disabled={
                    controlsDisabled ||
                    !thresholdDirty ||
                    !thresholdValid ||
                    !thresholdChanged
                  }
                >
                  {thresholdBusy ? "应用中…" : "应用"}
                </button>
              </div>
            </form>
          </div>
        </section>
      ) : null}

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

      {data && data.poolAvailable > 0 && (data.fiveHour || data.sevenDay) ? (
        <div className="account-mini-meters summary-meters">
          {data.fiveHour ? (
            <ProgressBar
              value={data.fiveHour.remainingPercent}
              invertTone
              label={`可用号平均 5h ${data.fiveHour.remainingPercent.toFixed(0)}% · reset ${formatDuration(data.fiveHour.resetAfterSeconds)}`}
            />
          ) : null}
          {data.sevenDay ? (
            <ProgressBar
              value={data.sevenDay.remainingPercent}
              invertTone
              label={`可用号平均 7d ${data.sevenDay.remainingPercent.toFixed(0)}% · reset ${formatDuration(data.sevenDay.resetAfterSeconds)}`}
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
              pendingAction={accountAction}
              disabled={controlsDisabled}
              onSelect={(selected) => void onSelect(selected)}
              onRecover={(recoverable) => void onRecover(recoverable)}
              onDelete={(deletable) => void onDelete(deletable)}
              officialProbe={officialProbes[account.id] ?? null}
              officialError={officialErrors[account.id] ?? null}
              officialBusy={officialBusyAccountId === account.id}
              onProbeOfficial={(probeAccount) => void onProbeOfficial(probeAccount)}
            />
          ))
        ) : data ? (
          <p className="muted-line">
            没有 OAuth/GPT 号池账号（AIHub/AnyRouter 等中转站请看「供应商」卡）
          </p>
        ) : null}
      </div>
    </CardShell>
  );
}
