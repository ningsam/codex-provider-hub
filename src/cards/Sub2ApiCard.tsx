import { useCallback, useEffect, useState } from "react";
import { CardShell } from "../components/CardShell";
import { ProgressBar } from "../components/ProgressBar";
import { api, REFRESH_MS } from "../lib/api";
import { formatDuration } from "../lib/format";
import type { Sub2ApiUsage } from "../types";

export function Sub2ApiCard() {
  const [data, setData] = useState<Sub2ApiUsage | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

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

  return (
    <CardShell
      title="Sub2API 号池"
      subtitle="OpenAI account pool quotas"
      refreshedAt={data?.fetchedAt}
      onRefresh={() => void refresh()}
      refreshing={busy}
    >
      {error ? <p className="error-line">{error}</p> : null}
      <div className="metric-row">
        <div>
          <div className="metric-label">5 小时剩余</div>
          <div className="metric-value">
            {data ? `${data.fiveHour.remainingPercent.toFixed(1)}%` : "—"}
          </div>
        </div>
        <div>
          <div className="metric-label">可用账号</div>
          <div className="metric-value mono">
            {data ? `${data.poolAvailable}/${data.poolTotal}` : "—"}
          </div>
        </div>
      </div>
      {data ? (
        <>
          <ProgressBar
            value={data.fiveHour.remainingPercent}
            invertTone
            label={`5h · reset ${formatDuration(data.fiveHour.resetAfterSeconds)}`}
          />
          <ProgressBar
            value={data.sevenDay.remainingPercent}
            invertTone
            label={`7d · reset ${formatDuration(data.sevenDay.resetAfterSeconds)}`}
          />
        </>
      ) : null}
    </CardShell>
  );
}
