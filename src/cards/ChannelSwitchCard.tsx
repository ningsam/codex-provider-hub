import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useState } from "react";
import { CardShell } from "../components/CardShell";

export type CodexChannel = "official" | "sub2api" | "mixed";

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
        `切换到「${name}」？\nHub 会先备份 auth.json 与 config.toml，完成后需要重启 Codex。`,
      )
    ) {
      return;
    }
    setBusy(true);
    setError(null);
    setHint(null);
    try {
      const result = await invoke<ChannelSwitchResult>(
        "switch_codex_channel",
        { target },
      );
      setStatus(result.status);
      setRestartReady(true);
      setHint(`${result.message}（已生成两份带时间戳备份）`);
      setRefreshedAt(new Date().toISOString());
    } catch (err) {
      setError(readableError(err));
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

      <dl className="kv-grid">
        <div>
          <dt>当前通道</dt>
          <dd>{status ? labels[current] : "—"}</dd>
        </div>
        <div>
          <dt>模型</dt>
          <dd className="mono">{status?.model || "—"}</dd>
        </div>
        <div>
          <dt>认证</dt>
          <dd className="mono">{status?.authMode || "—"}</dd>
        </div>
      </dl>

      <p className="muted-line">
        Profiles：官方 {status?.officialProfileSaved ? "已保存" : "待首次切换"}
        {" · "}网关 {status?.sub2apiProfileSaved ? "已保存" : "待首次切换"}
      </p>

      <div className="card-inline-actions">
        <button
          type="button"
          className="btn ghost"
          disabled={
            busy || (current === "official" && status?.configConsistent === true)
          }
          onClick={() => void switchTo("official")}
        >
          切到官方直连
        </button>
        <button
          type="button"
          className="btn primary"
          disabled={
            busy || (current === "sub2api" && status?.configConsistent === true)
          }
          onClick={() => void switchTo("sub2api")}
        >
          切到 Sub2API
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
