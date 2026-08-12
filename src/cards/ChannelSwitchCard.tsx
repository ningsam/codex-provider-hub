import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useState } from "react";
import { CardShell } from "../components/CardShell";

export type CodexChannel = "official" | "sub2api" | "mixed";

export interface OfficialAccountInfo {
  email: string;
  name: string | null;
  picture: string | null;
  hasActiveSubscription: boolean;
  subscriptionPlan: string | null;
}

export interface ChannelSwitchStatus {
  current: CodexChannel;
  modelProvider: string;
  model: string;
  authMode: string;
  preferredAuthMethod: string;
  officialProfileSaved: boolean;
  sub2apiProfileSaved: boolean;
  lastSwitchedAt: string | null;
  configConsistent: boolean;
  officialAccount: OfficialAccountInfo | null;
}

interface ChannelSwitchResult {
  status: ChannelSwitchStatus;
  authBackupPath: string;
  configBackupPath: string;
  message: string;
}

interface CodexRestartResult {
  application: string;
  reopened: boolean;
  message: string;
}

const labels: Record<CodexChannel, string> = {
  official: "官方直连",
  sub2api: "Sub2API",
  mixed: "配置不一致",
};

function readableError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function ChannelSwitchCard() {
  const [status, setStatus] = useState<ChannelSwitchStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [restartReady, setRestartReady] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [hint, setHint] = useState<string | null>(null);
  const [refreshedAt, setRefreshedAt] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      const next = await invoke<ChannelSwitchStatus>(
        "get_channel_switch_status",
      );
      setStatus(next);
      setRefreshedAt(new Date().toISOString());
    } catch (err) {
      setError(readableError(err));
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const switchTo = async (target: Exclude<CodexChannel, "mixed">) => {
    const name = labels[target];
    if (
      !window.confirm(
        `切换到「${name}」？\n\nHub 会先备份 auth.json 与 config.toml，完成后自动重启 Codex 应用，立即可用。`,
      )
    ) {
      return;
    }
    setBusy(true);
    setError(null);
    setHint(null);
    try {
      // Step 1: Switch channel
      const result = await invoke<ChannelSwitchResult>(
        "switch_codex_channel",
        { target },
      );
      setStatus(result.status);
      setHint(`${result.message}（已生成两份带时间戳备份）`);
      setRefreshedAt(new Date().toISOString());

      // Step 2: Auto-restart Codex
      await new Promise(resolve => setTimeout(resolve, 500)); // Brief pause
      setHint("正在重启 Codex 应用...");

      const restartResult = await invoke<CodexRestartResult>("restart_codex_app");
      setHint(`✓ 切换完成：${result.message}\n✓ ${restartResult.message}\n\n现在可以在 Codex 中使用新渠道。`);
      setRestartReady(false);

      // Auto-refresh status after restart
      await new Promise(resolve => setTimeout(resolve, 2000));
      await refresh();
    } catch (err) {
      setError(readableError(err));
      setRestartReady(true); // Allow manual restart on error
    } finally {
      setBusy(false);
    }
  };

  const restart = async () => {
    setBusy(true);
    setError(null);
    setHint(null);
    try {
      const result = await invoke<CodexRestartResult>("restart_codex_app");
      setHint(result.message);
      setRestartReady(false);
    } catch (err) {
      setError(readableError(err));
    } finally {
      setBusy(false);
    }
  };

  const current = status?.current ?? "mixed";
  const isOfficial = current === "official";
  const isSub2api = current === "sub2api";

  return (
    <CardShell
      className="card-span-2"
      title="Codex 通道切换"
      subtitle="官方 ChatGPT OAuth ⇆ Sub2API 网关 · 凭据仅在本机配置文件内切换"
      titleBadge={
        status ? (
          <span
            className={`guard-title-badge ${
              status.configConsistent ? "is-ok" : "is-danger"
            }`}
          >
            {labels[current]}
          </span>
        ) : null
      }
      refreshedAt={refreshedAt}
      onRefresh={() => void refresh()}
      refreshing={busy}
    >
      {error ? (
        <p className="error-line" role="alert">
          {error}
        </p>
      ) : null}
      {hint ? (
        <p className="hint-line" role="status">
          {hint}
        </p>
      ) : null}
      {status && !status.configConsistent ? (
        <p className="warn-line">
          model_provider、auth_mode 或模型前缀不一致；请选择目标通道完成修复。
        </p>
      ) : null}

      {/* Current Channel Display */}
      <div style={{
        padding: '16px',
        background: isOfficial ? '#e8f5e9' : isSub2api ? '#e3f2fd' : '#fff3e0',
        borderRadius: '8px',
        marginBottom: '16px',
        border: `2px solid ${isOfficial ? '#4caf50' : isSub2api ? '#2196f3' : '#ff9800'}`
      }}>
        <div style={{ fontSize: '14px', fontWeight: 600, marginBottom: '8px', color: '#666' }}>
          🎯 当前使用渠道
        </div>
        <div style={{ fontSize: '20px', fontWeight: 700, marginBottom: '4px' }}>
          {labels[current]}
        </div>
        <div className="mono" style={{ fontSize: '13px', color: '#666' }}>
          {status?.model || "—"}
        </div>
      </div>

      {/* Official Account Info */}
      {status?.officialAccount && isOfficial ? (
        <div style={{
          padding: '16px',
          background: '#f5f5f5',
          borderRadius: '8px',
          marginBottom: '16px'
        }}>
          <div style={{ fontSize: '14px', fontWeight: 600, marginBottom: '12px', color: '#666' }}>
            👤 ChatGPT 官方账号
          </div>
          <dl className="kv-grid">
            <div>
              <dt>邮箱</dt>
              <dd className="mono" style={{ fontSize: '13px' }}>
                {status.officialAccount.email}
              </dd>
            </div>
            {status.officialAccount.name ? (
              <div>
                <dt>姓名</dt>
                <dd>{status.officialAccount.name}</dd>
              </div>
            ) : null}
            <div>
              <dt>订阅状态</dt>
              <dd>
                {status.officialAccount.hasActiveSubscription ? (
                  <span style={{ color: '#4caf50', fontWeight: 600 }}>
                    ✓ {status.officialAccount.subscriptionPlan || 'Active'}
                  </span>
                ) : (
                  <span style={{ color: '#999' }}>Free</span>
                )}
              </dd>
            </div>
          </dl>
        </div>
      ) : null}

      <dl className="kv-grid">
        <div>
          <dt>认证模式</dt>
          <dd className="mono">{status?.authMode || "—"}</dd>
        </div>
        <div>
          <dt>模型提供商</dt>
          <dd className="mono">{status?.modelProvider || "—"}</dd>
        </div>
      </dl>

      <p className="muted-line">
        Profiles：官方 {status?.officialProfileSaved ? "✓" : "待首次切换"}
        {" · "}网关 {status?.sub2apiProfileSaved ? "✓" : "待首次切换"}
      </p>

      <div className="card-inline-actions">
        <button
          type="button"
          className={isOfficial && status?.configConsistent ? "btn primary" : "btn ghost"}
          disabled={
            busy || (current === "official" && status?.configConsistent === true)
          }
          onClick={() => void switchTo("official")}
        >
          {isOfficial ? "✓ 官方直连" : "切到官方直连"}
        </button>
        <button
          type="button"
          className={isSub2api && status?.configConsistent ? "btn primary" : "btn ghost"}
          disabled={
            busy || (current === "sub2api" && status?.configConsistent === true)
          }
          onClick={() => void switchTo("sub2api")}
        >
          {isSub2api ? "✓ Sub2API" : "切到 Sub2API"}
        </button>
        <button
          type="button"
          className={restartReady ? "btn danger" : "btn ghost"}
          disabled={busy}
          onClick={() => void restart()}
        >
          安全重启 Codex
        </button>
      </div>
      <p className="muted-line">
        切换会原子更新两份文件并保留用途+时间戳备份；重启只请求正常退出，不会强杀进程。
      </p>
    </CardShell>
  );
}
