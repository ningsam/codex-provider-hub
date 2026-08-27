import type { ReactNode } from "react";
import { formatRelativeRefresh } from "../lib/format";

function RefreshIcon({ spinning = false }: { spinning?: boolean }) {
  return (
    <svg
      className={spinning ? "refresh-icon is-spinning" : "refresh-icon"}
      width="15"
      height="15"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      <path d="M20 11a8 8 0 1 0-2.3 5.7" />
      <path d="M20 4v7h-7" />
    </svg>
  );
}

export function CardShell({
  title,
  subtitle,
  index,
  titleBadge,
  refreshedAt,
  onRefresh,
  refreshing,
  actions,
  children,
  className = "",
}: {
  title: string;
  subtitle?: string;
  /** Optional section index, e.g. "01" */
  index?: string;
  /** Optional status chip rendered beside the title */
  titleBadge?: ReactNode;
  refreshedAt?: string | null;
  onRefresh?: () => void;
  refreshing?: boolean;
  actions?: ReactNode;
  children: ReactNode;
  className?: string;
}) {
  return (
    <section className={`dash-card ${className}`.trim()}>
      <span className="card-specular" aria-hidden />
      <span className="card-refraction" aria-hidden />

      <header className="card-head">
        <div className="card-head-copy">
          <div className="card-heading-line">
            {index ? (
              <span className="card-index">{index}</span>
            ) : (
              <span className="card-index card-index-dot" aria-hidden>
                <span />
              </span>
            )}
            <div className="card-title-wrap">
              <div className="card-title-row">
                <h2>{title}</h2>
                {titleBadge}
              </div>
              {subtitle ? <p className="card-sub">{subtitle}</p> : null}
            </div>
          </div>
        </div>

        <div className="card-head-actions">
          {actions}
          {onRefresh ? (
            <button
              type="button"
              className="btn ghost refresh-btn"
              onClick={onRefresh}
              disabled={refreshing}
              aria-label={`刷新${title}`}
            >
              <RefreshIcon spinning={refreshing} />
              <span>{refreshing ? "刷新中" : "刷新"}</span>
            </button>
          ) : null}
        </div>
      </header>

      <div className="card-body">{children}</div>

      <footer className="card-foot">
        <span className="card-foot-status" aria-hidden>
          <span />
        </span>
        <span>Updated {formatRelativeRefresh(refreshedAt)}</span>
      </footer>
    </section>
  );
}
