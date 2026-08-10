import type { ReactNode } from "react";
import { formatRelativeRefresh } from "../lib/format";

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
  /** Optional cine-style section index, e.g. "01" */
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
      <header className="card-head">
        <div className="card-head-copy">
          {index ? <p className="card-index">{index}</p> : null}
          <div className="card-title-row">
            <h2>{title}</h2>
            {titleBadge}
          </div>
          {subtitle ? <p className="card-sub">{subtitle}</p> : null}
        </div>
        <div className="card-head-actions">
          {actions}
          {onRefresh ? (
            <button
              type="button"
              className="btn ghost"
              onClick={onRefresh}
              disabled={refreshing}
              aria-label={`Refresh ${title}`}
            >
              {refreshing ? "…" : "Refresh"}
            </button>
          ) : null}
        </div>
      </header>
      <div className="card-body">{children}</div>
      <footer className="card-foot">
        <span>Updated {formatRelativeRefresh(refreshedAt)}</span>
      </footer>
    </section>
  );
}
