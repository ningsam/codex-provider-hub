import { useCallback, useEffect, useMemo, useState, type FormEvent } from "react";
import { CardShell } from "../components/CardShell";
import { useI18n } from "../i18n";
import { api, REFRESH_MS } from "../lib/api";
import type { ProviderInfo } from "../types";

function slugify(name: string): string {
  return (
    name
      .trim()
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .slice(0, 32) || "provider"
  );
}

export function ProvidersCard() {
  const { t } = useI18n();
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

  const onAdd = async (event: FormEvent) => {
    event.preventDefault();
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
      setHint(result.hint ?? t("providers.synced", { count: result.modelsSynced }));
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
      setHint(t("providers.probed", { count: ids.length }));
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
          t("providers.syncHint", {
            name: result.provider.name,
            count: result.modelsSynced,
          }),
      );
      await refresh();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      setBusy(false);
    }
  };

  const onRemove = async (provider: ProviderInfo) => {
    const ok = window.confirm(
      t("providers.deleteConfirm", {
        name: provider.name,
        prefix: provider.prefix || "?",
      }),
    );
    if (!ok) return;
    setBusy(true);
    setError(null);
    setHint(null);
    try {
      await api.removeProvider(provider.id);
      setHint(t("providers.removed", { name: provider.name }));
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
      title={t("providers.title")}
      subtitle={t("providers.subtitle")}
      onRefresh={() => void refresh()}
      refreshing={busy}
      actions={
        <button
          type="button"
          className="btn primary"
          onClick={() => {
            setShowForm((value) => !value);
            setError(null);
          }}
          disabled={busy}
        >
          {showForm ? t("common.cancel") : t("providers.add")}
        </button>
      }
    >
      {error ? <p className="error-line">{error}</p> : null}
      {hint ? <p className="hint-line">{hint}</p> : null}

      {showForm ? (
        <form className="provider-form" onSubmit={(event) => void onAdd(event)}>
          <label>
            {t("providers.displayName")}
            <input
              value={name}
              onChange={(event) => {
                setName(event.target.value);
                if (!prefixTouched) setPrefix(slugify(event.target.value));
              }}
              placeholder="My Relay"
              required
              autoComplete="off"
            />
          </label>
          <label>
            {t("providers.baseUrl")}
            <input
              value={baseUrl}
              onChange={(event) => setBaseUrl(event.target.value)}
              placeholder="https://xxx.example.com/v1"
              required
              autoComplete="off"
              spellCheck={false}
            />
          </label>
          <label>
            {t("providers.apiKey")}
            <input
              type="password"
              value={apiKey}
              onChange={(event) => setApiKey(event.target.value)}
              placeholder="sk-…"
              required
              autoComplete="off"
            />
          </label>
          <label>
            {t("providers.prefix")}
            <input
              value={prefixTouched ? prefix : derivedPrefix}
              onChange={(event) => {
                setPrefixTouched(true);
                setPrefix(event.target.value.toLowerCase());
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
              onChange={(event) => setProbeModels(event.target.checked)}
            />
            {t("providers.syncOnAdd")}
          </label>
          <div className="form-actions">
            <button
              type="button"
              className="btn ghost"
              disabled={busy || !baseUrl || !apiKey}
              onClick={() => void onProbe()}
            >
              {t("providers.probeOnly")}
            </button>
            <button type="submit" className="btn primary" disabled={busy}>
              {t("providers.addProvider")}
            </button>
          </div>
          {probed ? (
            <p className="probe-preview mono">
              {probed.slice(0, 12).join(", ")}
              {probed.length > 12 ? ` … +${probed.length - 12}` : ""}
            </p>
          ) : null}
          <p className="form-note">{t("providers.formNote")}</p>
        </form>
      ) : null}

      {providers.length === 0 ? (
        <p className="empty-hint">{t("providers.empty")}</p>
      ) : (
        <div className="provider-list">
          {providers.map((provider) => (
            <article key={provider.id} className="provider-row">
              <div className="provider-top">
                <div>
                  <div className="provider-name">{provider.name}</div>
                  <div className="provider-meta mono">
                    {provider.baseUrlMasked || provider.baseUrl} · {provider.prefix || "—"}-
                  </div>
                </div>
                <div
                  className={`provider-status ${
                    provider.status === "active" ? "is-on" : "is-off"
                  }`}
                >
                  {provider.status === "active" ? t("common.active") : provider.status}
                </div>
              </div>
              <dl className="kv-grid provider-kv">
                <div>
                  <dt>{t("providers.models")}</dt>
                  <dd className="mono">{provider.modelCount}</dd>
                </div>
                <div>
                  <dt>{t("providers.key")}</dt>
                  <dd>
                    {provider.hasApiKey
                      ? t("providers.configured")
                      : t("providers.missing")}
                  </dd>
                </div>
                <div>
                  <dt>{t("providers.scheduling")}</dt>
                  <dd>{provider.schedulable ? t("common.on") : t("common.off")}</dd>
                </div>
              </dl>
              {provider.errorMessage ? (
                <p className="error-line">{provider.errorMessage}</p>
              ) : null}
              <div className="provider-actions">
                <button
                  type="button"
                  className="btn ghost"
                  disabled={busy}
                  onClick={() => void onSync(provider.id)}
                >
                  {t("providers.probeSync")}
                </button>
                <button
                  type="button"
                  className="btn danger"
                  disabled={busy}
                  onClick={() => void onRemove(provider)}
                >
                  {t("common.delete")}
                </button>
              </div>
            </article>
          ))}
        </div>
      )}
    </CardShell>
  );
}
