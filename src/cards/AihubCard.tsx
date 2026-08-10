import { useCallback, useEffect, useState } from "react";
import { CardShell } from "../components/CardShell";
import { ProgressBar } from "../components/ProgressBar";
import { api, REFRESH_MS } from "../lib/api";
import type { AihubBalance } from "../types";

export function AihubCard() {
  const [data, setData] = useState<AihubBalance | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

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

  const total = data ? data.balance + data.used : 0;
  const remainingPct = data && total > 0 ? (data.balance / total) * 100 : data ? 100 : 0;

  return (
    <CardShell
      title="中转站 AIHub"
      subtitle="Relay station wallet"
      refreshedAt={data?.fetchedAt}
      onRefresh={() => void refresh()}
      refreshing={busy}
    >
      {error ? <p className="error-line">{error}</p> : null}
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
    </CardShell>
  );
}
