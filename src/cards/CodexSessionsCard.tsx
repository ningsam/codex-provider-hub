import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useState } from "react";
import { CardShell } from "../components/CardShell";
import "./CodexSessionsCard.css";

export interface CodexThreadSummary {
  id: string;
  cwd: string;
  title: string;
  updatedAt: number;
  modelProvider: string;
  tokensUsed: number;
  archived: boolean;
}

export interface CodexSessionsSnapshot {
  threads: CodexThreadSummary[];
  totalCount: number;
  archivedCount: number;
  currentProvider: string | null;
  databasePath: string;
  codexRunning: boolean;
  blockingProcesses: string[];
  mergeReady: boolean;
  mergeBlockedReason: string | null;
  fetchedAt: string;
}

export interface CodexSessionsMergeResult {
  currentProvider: string;
  updatedCount: number;
  totalCount: number;
  backupPath: string | null;
  message: string;
}

const ALL = "__all__";

function displayProject(cwd: string): string {
  const parts = cwd.split("/").filter(Boolean);
  const leaf = parts[parts.length - 1] ?? cwd;
  return `${leaf} — ${cwd}`;
}

function displayDate(timestampSeconds: number): string {
  if (!Number.isFinite(timestampSeconds)) return "未知时间";
  const date = new Date(timestampSeconds * 1000);
  if (Number.isNaN(date.getTime())) return "未知时间";
  return new Intl.DateTimeFormat("zh-CN", {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  }).format(date);
}

function isoDate(timestampSeconds: number): string | undefined {
  if (!Number.isFinite(timestampSeconds)) return undefined;
  const date = new Date(timestampSeconds * 1000);
  return Number.isNaN(date.getTime()) ? undefined : date.toISOString();
}

function displayTokens(tokens: number): string {
  if (!Number.isFinite(tokens)) return "—";
  return new Intl.NumberFormat("zh-CN", {
    notation: "compact",
    maximumFractionDigits: 1,
  }).format(tokens);
}

function providerTone(provider: string): string {
  const normalized = provider.toLowerCase();
  if (normalized === "openai") return "is-openai";
  if (normalized === "sub2api") return "is-sub2api";
  if (normalized === "anyrouter") return "is-anyrouter";
  return "is-other";
}

function ThreadList({ threads }: { threads: CodexThreadSummary[] }) {
  if (threads.length === 0) {
    return <p className="empty-hint">当前过滤条件下没有会话。</p>;
  }

  return (
    <div className="session-browser-list">
      {threads.map((thread) => (
        <article className="session-browser-row" key={thread.id}>
          <div className="session-browser-main">
            <div className="session-browser-title-row">
              <span
                className={`session-provider-tag ${providerTone(thread.modelProvider)}`}
              >
                {thread.modelProvider}
              </span>
              <time dateTime={isoDate(thread.updatedAt)}>
                {displayDate(thread.updatedAt)}
              </time>
            </div>
            <p className="session-browser-title" title={thread.title}>
              {thread.title || "未命名会话"}
            </p>
            <p className="session-browser-path mono" title={thread.cwd}>
              {thread.cwd}
            </p>
          </div>
          <div className="session-browser-tokens" title={`${thread.tokensUsed} tokens`}>
            <strong>{displayTokens(thread.tokensUsed)}</strong>
            <span>tokens</span>
          </div>
        </article>
      ))}
    </div>
  );
}

export function CodexSessionsCard() {
  const [snapshot, setSnapshot] = useState<CodexSessionsSnapshot | null>(null);
  const [project, setProject] = useState(ALL);
  const [provider, setProvider] = useState(ALL);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [hint, setHint] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      const next = await invoke<CodexSessionsSnapshot>("list_codex_sessions");
      setSnapshot(next);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const projects = useMemo(
    () =>
      Array.from(new Set(snapshot?.threads.map((thread) => thread.cwd) ?? [])).sort(
        (left, right) => left.localeCompare(right),
      ),
    [snapshot],
  );
  const providers = useMemo(
    () =>
      Array.from(
        new Set(snapshot?.threads.map((thread) => thread.modelProvider) ?? []),
      ).sort((left, right) => left.localeCompare(right)),
    [snapshot],
  );

  const filtered = useMemo(
    () =>
      (snapshot?.threads ?? []).filter(
        (thread) =>
          (project === ALL || thread.cwd === project) &&
          (provider === ALL || thread.modelProvider === provider),
      ),
    [project, provider, snapshot],
  );
  const active = filtered.filter((thread) => !thread.archived);
  const archived = filtered.filter((thread) => thread.archived);

  const mergeAll = async () => {
    if (!snapshot?.currentProvider || !snapshot.mergeReady) return;
    const confirmed = window.confirm(
      `将全部 ${snapshot.totalCount} 条会话的 provider 改为「${snapshot.currentProvider}」？\n\nHub 会先备份 state_5.sqlite。请确认 Codex/ChatGPT 已完全退出。`,
    );
    if (!confirmed) return;

    setBusy(true);
    setError(null);
    setHint(null);
    try {
      const result = await invoke<CodexSessionsMergeResult>(
        "merge_codex_sessions_into_current_provider",
      );
      setHint(
        result.backupPath
          ? `${result.message} 备份：${result.backupPath}`
          : result.message,
      );
      await refresh();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
      setBusy(false);
    }
  };

  return (
    <CardShell
      className="card-span-full codex-sessions-card"
      title="统一会话浏览器"
      subtitle="跨 provider 查看全部 Codex 历史；归档会话默认折叠。"
      index="07"
      refreshedAt={snapshot?.fetchedAt ?? null}
      onRefresh={() => void refresh()}
      refreshing={busy}
      actions={
        <button
          type="button"
          className="btn danger"
          disabled={busy || !snapshot?.mergeReady}
          onClick={() => void mergeAll()}
          title={snapshot?.mergeBlockedReason ?? undefined}
        >
          全部并入当前分区
        </button>
      }
    >
      {error ? <p className="error-line">{error}</p> : null}
      {hint ? <p className="hint-line session-browser-result">{hint}</p> : null}

      {snapshot ? (
        <>
          <div className="session-browser-summary">
            <div>
              <span>全部会话</span>
              <strong>{snapshot.totalCount}</strong>
            </div>
            <div>
              <span>当前分区</span>
              <strong className="mono">{snapshot.currentProvider ?? "未知"}</strong>
            </div>
            <div>
              <span>已归档</span>
              <strong>{snapshot.archivedCount}</strong>
            </div>
          </div>

          {snapshot.mergeBlockedReason ? (
            <p className={snapshot.codexRunning ? "warn-line" : "error-line"}>
              {snapshot.mergeBlockedReason}
            </p>
          ) : (
            <p className="muted-line">
              Codex 已退出；合并时将先创建 SQLite 一致性备份，再执行单事务更新。
            </p>
          )}

          <div className="session-browser-filters">
            <label>
              项目路径
              <select value={project} onChange={(event) => setProject(event.target.value)}>
                <option value={ALL}>全部项目</option>
                {projects.map((cwd) => (
                  <option key={cwd} value={cwd}>
                    {displayProject(cwd)}
                  </option>
                ))}
              </select>
            </label>
            <label>
              Provider
              <select value={provider} onChange={(event) => setProvider(event.target.value)}>
                <option value={ALL}>全部 provider</option>
                {providers.map((name) => (
                  <option key={name} value={name}>
                    {name}
                  </option>
                ))}
              </select>
            </label>
            <p>
              显示 {filtered.length} / {snapshot.totalCount}
            </p>
          </div>

          <ThreadList threads={active} />

          <details className="session-browser-archived">
            <summary>已归档会话（{archived.length}）</summary>
            <ThreadList threads={archived} />
          </details>
        </>
      ) : busy ? (
        <p className="empty-hint">正在只读载入 Codex 会话…</p>
      ) : null}
    </CardShell>
  );
}
