import { clampPercent } from "../lib/format";

type Tone = "ok" | "warn" | "danger" | "accent";

/** `highIsGood`: remaining quotas. Otherwise treat high % as usage pressure. */
function toneFor(percent: number, highIsGood: boolean): Tone {
  if (highIsGood) {
    if (percent >= 70) return "ok";
    if (percent >= 35) return "warn";
    return "danger";
  }
  if (percent >= 70) return "danger";
  if (percent >= 35) return "warn";
  return "ok";
}

export function ProgressBar({
  value,
  invertTone = false,
  label,
}: {
  value: number;
  /** When true, high remaining % is good (remaining quotas). */
  invertTone?: boolean;
  label?: string;
}) {
  const pct = clampPercent(value);
  const tone = toneFor(pct, invertTone);

  return (
    <div className="progress-block">
      {label ? (
        <div className="progress-meta">
          <span>{label}</span>
          <span className="mono">{pct.toFixed(1)}%</span>
        </div>
      ) : null}
      <div className="progress-track" role="progressbar" aria-valuenow={pct} aria-valuemin={0} aria-valuemax={100}>
        <div className={`progress-fill tone-${tone}`} style={{ width: `${pct}%` }} />
      </div>
    </div>
  );
}
