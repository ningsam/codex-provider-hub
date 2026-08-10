import { useCallback, useEffect, useState, type FormEvent } from "react";
import { CardShell } from "../components/CardShell";
import { ProgressBar } from "../components/ProgressBar";
import { api, REFRESH_MS } from "../lib/api";
import type { AihubBalance } from "../types";

export function AihubCard() {
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
    // Stagger first fetch slightly so cards don't stampede.
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
      const bal = await api.setAihubApiKey(apiKey);
      setData(bal);
      setApiKey("");
      setShowKeyForm(false);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const onClearKey = async () => {
    if (!window.confirm("清除本地保存的 AIHub Key？将回退到 Sub2API / 环境变量。")) {
      return;
    }
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
      title="中转站 AIHub"
      subtitle="Relay station wallet"
      refreshedAt={data?.fetchedAt}
      onRefresh={() => void refresh()}
      refreshing={busy}
      actions={
        <>
          <button
            type="button"
            className="btn ghost"
            disabled={busy}
            onClick={() => setShowKeyForm((v) => !v)}
          >
            {showKeyForm ? "取消" : "设置 Key"}
          </button>
          {data?.hasStoredKey ? (
            <button
              type="button"
              className="btn ghost danger-text"
              disabled={busy}
              onClick={() => void onClearKey()}
            >
              清除 Key
            </button>
          ) : null}
        </>
      }
    >
      {error ? <p className="error-line">{error}</p> : null}

      {showKeyForm ? (
        <form className="provider-form" onSubmit={(e) => void onSaveKey(e)}>
          <label>
            AIHub API Key
            <input
              type="password"
              value={apiKey}
              onChange={(e) => setApiKey(e.target.value)}
              placeholder="sk-…"
              required
              autoComplete="off"
              spellCheck={false}
            />
          </label>
          <div className="form-actions">
            <button type="submit" className="btn primary" disabled={busy || !apiKey.trim()}>
              保存并验证
            </button>
          </div>
          <p className="form-note">Key 加密写入本机 app data；默认也会尝试从 Sub2API AIHub 账号同步。</p>
        </form>
      ) : null}

      <div className="metric-row">
        <div>
          <div className="metric-label">余额</div>
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
          <div className="metric-label">今日已用</div>
          <div className="metric-value mono">
            {data ? data.used.toFixed(2) : "—"}
          </div>
        </div>
      </div>
      {data ? (
        <ProgressBar value={remainingPct} invertTone label="余额占比（余额 / 余额+今日）" />
      ) : null}
      {data?.keySource ? (
        <p className="muted-line mono">源: {data.keySource}</p>
      ) : null}
    </CardShell>
  );
}
