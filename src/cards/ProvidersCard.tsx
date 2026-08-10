import { useCallback, useEffect, useMemo, useState, type FormEvent } from "react";
import { CardShell } from "../components/CardShell";
import { api, REFRESH_MS } from "../lib/api";
import type { ProviderInfo } from "../types";

function slugify(name: string): string {
  return name
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 32) || "provider";
}

export function ProvidersCard() {
  const [providers, setProviders] = useState<ProviderInfo[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [hint, setHint] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [showForm, setShowForm] = useState(false);

  const [name, setName] = useState("");
  const [baseUrl, setBaseUrl] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [prefix, setPrefix] = useState("");
  const [prefixTouched, setPrefixTouched] = useState(false);
  const [probeModels, setProbeModels] = useState(true);
  const [probed, setProbed] = useState<string[] | null>(null);

  const derivedPrefix = useMemo(() => slugify(name), [name]);

  const refresh = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      setProviders(await api.listProviders());
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    const boot = window.setTimeout(() => void refresh(), 200);
    const id = window.setInterval(() => void refresh(), REFRESH_MS.providers);
    return () => {
      window.clearTimeout(boot);
      window.clearInterval(id);
    };
  }, [refresh]);

  const resetForm = () => {
    setName("");
    setBaseUrl("");
    setApiKey("");
    setPrefix("");
    setPrefixTouched(false);
    setProbeModels(true);
    setProbed(null);
  };

  const onAdd = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    setHint(null);
    try {
      const result = await api.addProvider({
        name,
        baseUrl,
        apiKey,
        prefix: (prefixTouched ? prefix : derivedPrefix) || undefined,
        probeModels,
      });
      setApiKey("");
      setHint(result.hint ?? `已同步 ${result.modelsSynced} 个模型`);
      setShowForm(false);
      resetForm();
      await refresh();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      setBusy(false);
    }
  };

  const onProbe = async () => {
    setBusy(true);
    setError(null);
    setHint(null);
    try {
      const ids = await api.probeProviderModels(baseUrl, apiKey);
      setProbed(ids);
      setHint(`探测到 ${ids.length} 个模型`);
    } catch (err) {
      setProbed(null);
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const onSync = async (id: number) => {
    setBusy(true);
    setError(null);
    setHint(null);
    try {
      const result = await api.syncProviderModels(id);
      setHint(
        result.hint ??
          `已同步 ${result.provider.name} · ${result.modelsSynced} 模型`,
      );
      await refresh();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      setBusy(false);
    }
  };

  const onRemove = async (p: ProviderInfo) => {
    const ok = window.confirm(
      `删除供应商「${p.name}」？\n将移除 Sub2API 账号并清理 catalog 中前缀 ${p.prefix || "?"} 的模型。`,
    );
    if (!ok) return;
    setBusy(true);
    setError(null);
    setHint(null);
    try {
      await api.removeProvider(p.id);
      setHint(`已删除 ${p.name}`);
      await refresh();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      setBusy(false);
    }
  };

  return (
    <CardShell
      className="card-span-2"
      index="05"
      title="供应商"
      subtitle="OpenAI 兼容中转 · Sub2API apikey · 同步 Codex catalog"
      onRefresh={() => void refresh()}
      refreshing={busy}
      actions={
        <button
          type="button"
          className="btn primary"
          onClick={() => {
            setShowForm((v) => !v);
            setError(null);
          }}
          disabled={busy}
        >
          {showForm ? "取消" : "添加"}
        </button>
      }
    >
      {error ? <p className="error-line">{error}</p> : null}
      {hint ? <p className="hint-line">{hint}</p> : null}

      {showForm ? (
        <form className="provider-form" onSubmit={(e) => void onAdd(e)}>
          <label>
            显示名
            <input
              value={name}
              onChange={(e) => {
                setName(e.target.value);
                if (!prefixTouched) setPrefix(slugify(e.target.value));
              }}
              placeholder="My Relay"
              required
              autoComplete="off"
            />
          </label>
          <label>
            Base URL
            <input
              value={baseUrl}
              onChange={(e) => setBaseUrl(e.target.value)}
              placeholder="https://xxx.example.com/v1"
              required
              autoComplete="off"
              spellCheck={false}
            />
          </label>
          <label>
            API Key
            <input
              type="password"
              value={apiKey}
              onChange={(e) => setApiKey(e.target.value)}
              placeholder="sk-…"
              required
              autoComplete="off"
            />
          </label>
          <label>
            模型前缀
            <input
              value={prefixTouched ? prefix : derivedPrefix}
              onChange={(e) => {
                setPrefixTouched(true);
                setPrefix(e.target.value.toLowerCase());
              }}
              placeholder={derivedPrefix}
              autoComplete="off"
              spellCheck={false}
            />
          </label>
          <label className="check-row">
            <input
              type="checkbox"
              checked={probeModels}
              onChange={(e) => setProbeModels(e.target.checked)}
            />
            添加时探测并同步模型
          </label>
          <div className="form-actions">
            <button
              type="button"
              className="btn ghost"
              disabled={busy || !baseUrl || !apiKey}
              onClick={() => void onProbe()}
            >
              仅探测
            </button>
            <button type="submit" className="btn primary" disabled={busy}>
              添加供应商
            </button>
          </div>
          {probed ? (
            <p className="probe-preview mono">
              {probed.slice(0, 12).join(", ")}
              {probed.length > 12 ? ` … +${probed.length - 12}` : ""}
            </p>
          ) : null}
          <p className="form-note">
            Key 仅用于创建 / 探测请求，不会写入前端存储。若出现 502 host not
            allowed，需在 Sub2API UpstreamHosts 放行该域名并重启网关。
          </p>
        </form>
      ) : null}

      {providers.length === 0 ? (
        <p className="empty-hint">暂无 apikey 供应商。添加后会同步到 Codex 模型列表。</p>
      ) : (
        <div className="provider-list">
          {providers.map((p) => (
            <article key={p.id} className="provider-row">
              <div className="provider-top">
                <div>
                  <div className="provider-name">{p.name}</div>
                  <div className="provider-meta mono">
                    {p.baseUrlMasked || p.baseUrl} · {p.prefix || "—"}-
                  </div>
                </div>
                <div
                  className={`provider-status ${
                    p.status === "active" ? "is-on" : "is-off"
                  }`}
                >
                  {p.status}
                </div>
              </div>
              <dl className="kv-grid provider-kv">
                <div>
                  <dt>模型</dt>
                  <dd className="mono">{p.modelCount}</dd>
                </div>
                <div>
                  <dt>Key</dt>
                  <dd>{p.hasApiKey ? "已配置" : "缺失"}</dd>
                </div>
                <div>
                  <dt>调度</dt>
                  <dd>{p.schedulable ? "on" : "off"}</dd>
                </div>
              </dl>
              {p.errorMessage ? (
                <p className="error-line">{p.errorMessage}</p>
              ) : null}
              <div className="provider-actions">
                <button
                  type="button"
                  className="btn ghost"
                  disabled={busy}
                  onClick={() => void onSync(p.id)}
                >
                  探测并同步模型
                </button>
                <button
                  type="button"
                  className="btn danger"
                  disabled={busy}
                  onClick={() => void onRemove(p)}
                >
                  删除
                </button>
              </div>
            </article>
          ))}
        </div>
      )}
    </CardShell>
  );
}
