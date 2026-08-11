import { useCallback, useEffect, useMemo, useState } from "react";
import { CardShell } from "../components/CardShell";
import { routeDoctorApi } from "../lib/routeDoctorApi";
import type {
  RouteDoctorIssue,
  RouteDoctorRepairAction,
  RouteDoctorRepairResult,
  RouteDoctorResult,
  RouteDoctorSeverity,
} from "../routeDoctorTypes";
import "./RouteDoctorCard.css";

const APPLY_PHRASE = "APPLY_ROUTE_DOCTOR_REPAIR";

const severityLabel: Record<RouteDoctorSeverity, string> = {
  critical: "阻断",
  warning: "风险",
  info: "提示",
};

function readableError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function displayValue(value: unknown): string {
  if (value === null) return "null";
  if (typeof value === "string") return value;
  return JSON.stringify(value);
}

function affectedLabel(issue: RouteDoctorIssue): string | null {
  const parts: string[] = [];
  if (issue.groupId !== null) parts.push(`group #${issue.groupId}`);
  if (issue.accountIds.length > 0) {
    parts.push(`accounts ${issue.accountIds.map((id) => `#${id}`).join(", ")}`);
  }
  return parts.length > 0 ? parts.join(" · ") : null;
}

export function RouteDoctorCard() {
  const [result, setResult] = useState<RouteDoctorResult | null>(null);
  const [preview, setPreview] = useState<RouteDoctorRepairResult | null>(null);
  const [confirmation, setConfirmation] = useState("");
  const [busy, setBusy] = useState<
    "diagnose" | "probe" | "preview" | "apply" | null
  >(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const diagnose = useCallback(async () => {
    setBusy("diagnose");
    setError(null);
    setNotice(null);
    try {
      const next = await routeDoctorApi.diagnose();
      setResult(next);
      setPreview(null);
      setConfirmation("");
    } catch (nextError) {
      setError(readableError(nextError));
    } finally {
      setBusy(null);
    }
  }, []);

  useEffect(() => {
    void diagnose();
  }, [diagnose]);

  const criticalCount = useMemo(
    () =>
      result?.report.issues.filter((issue) => issue.severity === "critical")
        .length ?? 0,
    [result],
  );

  const previewRepair = async (action: RouteDoctorRepairAction) => {
    setBusy("preview");
    setError(null);
    setNotice(null);
    try {
      const next = await routeDoctorApi.repair(action, false);
      setPreview(next);
      setConfirmation("");
    } catch (nextError) {
      setError(readableError(nextError));
    } finally {
      setBusy(null);
    }
  };

  const applyRepair = async () => {
    if (!preview || confirmation !== APPLY_PHRASE) return;
    setBusy("apply");
    setError(null);
    setNotice(null);
    try {
      const applied = await routeDoctorApi.repair(
        preview.plan.action,
        true,
        confirmation,
      );
      setNotice(applied.message);
      setPreview(null);
      setConfirmation("");
      const next = await routeDoctorApi.diagnose();
      setResult(next);
    } catch (nextError) {
      setError(readableError(nextError));
    } finally {
      setBusy(null);
    }
  };

  const probeResponses = async () => {
    if (
      !window.confirm(
        "最小 responses 探测会向每个中转站发送一次真实请求，并消耗少量额度。继续吗？",
      )
    ) {
      return;
    }
    setBusy("probe");
    setError(null);
    setNotice(null);
    try {
      const relayProbes = await routeDoctorApi.probeRelays(true);
      setResult((current) =>
        current ? { ...current, relayProbes, capturedAt: new Date().toISOString() } : current,
      );
      setNotice("中转站 /v1/models 与最小 /v1/responses 探测已完成。");
    } catch (nextError) {
      setError(readableError(nextError));
    } finally {
      setBusy(null);
    }
  };

  const report = result?.report;
  const healthy = report?.healthy === true;

  return (
    <CardShell
      className="card-span-2 route-doctor-card"
      title="Sub2API 路由医生"
      subtitle="503 排障链 · 默认只读诊断 · 修复前整库备份与双重审计"
      titleBadge={
        report ? (
          <span
            className={`route-doctor-health ${healthy ? "is-healthy" : "is-critical"}`}
          >
            {healthy ? "路由可用" : `${criticalCount} 项阻断`}
          </span>
        ) : null
      }
      refreshedAt={result?.capturedAt}
      onRefresh={() => void diagnose()}
      refreshing={busy === "diagnose"}
      actions={
        <button
          type="button"
          className="btn ghost route-doctor-probe-button"
          onClick={() => void probeResponses()}
          disabled={busy !== null || !result}
          title="会向每个中转站发送一次真实请求"
        >
          {busy === "probe" ? "探测中…" : "Responses 实测 · 消耗额度"}
        </button>
      }
    >
      <div className="route-doctor-safety" role="note">
        <span className="route-doctor-safety-dot" aria-hidden />
        <span>
          普通诊断只读 DB，并仅请求中转站 <code>/v1/models</code>；不会改 key、组或账号。
        </span>
      </div>

      {error ? (
        <p className="error-line" role="alert">
          {error}
        </p>
      ) : null}
      {notice ? (
        <p className="hint-line" role="status">
          {notice}
        </p>
      ) : null}

      <dl className="route-doctor-summary">
        <div>
          <dt>当前 key / group</dt>
          <dd className="mono">
            {report?.currentApiKeyId ? `#${report.currentApiKeyId}` : "—"}
            {" → "}
            {report?.currentGroupId ? `#${report.currentGroupId}` : "未绑定"}
          </dd>
          <span>{report?.currentGroupName ?? "尚未完成诊断"}</span>
        </div>
        <div>
          <dt>当前模型</dt>
          <dd className="mono route-doctor-model">{report?.currentModel ?? "—"}</dd>
          <span>逐账号核对 model_mapping</span>
        </div>
        <div>
          <dt>综合可用成员</dt>
          <dd className={healthy ? "is-ok-text" : "is-danger-text"}>
            {report ? report.usableMemberCount : "—"}
          </dd>
          <span>状态 · 停车 · 规则 · 映射</span>
        </div>
      </dl>

      <section className="route-doctor-section" aria-labelledby="route-findings-title">
        <div className="route-doctor-section-head">
          <div>
            <p className="route-doctor-kicker">Findings</p>
            <h3 id="route-findings-title">诊断发现</h3>
          </div>
          <span className="route-doctor-count">
            {report ? `${report.issues.length} 项` : "等待中"}
          </span>
        </div>

        {!report && !error ? <p className="empty-hint">正在读取安全快照…</p> : null}
        {report?.issues.length === 0 ? (
          <div className="route-doctor-empty">
            <span aria-hidden>✓</span>
            <div>
              <strong>未发现会导致 503 的配置问题</strong>
              <p>当前组有可调度成员、中转兜底和当前模型映射。</p>
            </div>
          </div>
        ) : null}

        <div className="route-doctor-findings">
          {report?.issues.map((issue) => {
            const affected = affectedLabel(issue);
            return (
              <article
                key={`${issue.code}-${issue.groupId ?? "none"}-${issue.accountIds.join("-")}`}
                className={`route-doctor-finding severity-${issue.severity}`}
              >
                <div className="route-doctor-finding-rail" aria-hidden />
                <div className="route-doctor-finding-copy">
                  <div className="route-doctor-finding-meta">
                    <span className={`route-doctor-severity severity-${issue.severity}`}>
                      {severityLabel[issue.severity]}
                    </span>
                    <code>{issue.code}</code>
                  </div>
                  <h4>{issue.title}</h4>
                  <p>{issue.detail}</p>
                  {affected ? <small className="mono">{affected}</small> : null}
                </div>
                <div className="route-doctor-finding-action">
                  {issue.repair ? (
                    <button
                      type="button"
                      className="btn ghost"
                      disabled={busy !== null}
                      onClick={() => void previewRepair(issue.repair!)}
                    >
                      {busy === "preview" ? "生成中…" : "预览修复"}
                    </button>
                  ) : (
                    <span>需人工确认</span>
                  )}
                </div>
              </article>
            );
          })}
        </div>
      </section>

      {preview ? (
        <section className="route-doctor-plan" aria-labelledby="route-plan-title">
          <div className="route-doctor-section-head">
            <div>
              <p className="route-doctor-kicker">Dry run</p>
              <h3 id="route-plan-title">修复预览</h3>
            </div>
            <button
              type="button"
              className="btn ghost"
              disabled={busy === "apply"}
              onClick={() => {
                setPreview(null);
                setConfirmation("");
              }}
            >
              取消
            </button>
          </div>
          <p className="route-doctor-plan-summary">{preview.plan.summary}</p>
          <div className="route-doctor-changes">
            {preview.plan.changes.map((change) => (
              <div key={`${change.entity}-${change.entityId}-${change.field}`}>
                <span className="mono">
                  {change.entity} #{change.entityId} · {change.field}
                </span>
                <code>
                  {displayValue(change.oldValue)} <b>→</b> {displayValue(change.newValue)}
                </code>
              </div>
            ))}
          </div>
          <p className="route-doctor-plan-warning">
            执行顺序固定：pg_dump 整库备份 → Hub 本地预审计 → Sub2API audit_logs →
            受控写入 → 仅重启 app 容器 → 状态复核。
          </p>
          <label className="route-doctor-confirm">
            <span>
              输入确认短语 <code>{APPLY_PHRASE}</code>
            </span>
            <input
              type="text"
              value={confirmation}
              autoComplete="off"
              spellCheck={false}
              onChange={(event) => setConfirmation(event.currentTarget.value)}
              disabled={busy === "apply"}
            />
          </label>
          <button
            type="button"
            className="btn danger route-doctor-apply"
            disabled={confirmation !== APPLY_PHRASE || busy === "apply"}
            onClick={() => void applyRepair()}
          >
            {busy === "apply" ? "备份、修复并复核中…" : "确认备份并执行修复"}
          </button>
        </section>
      ) : null}

      <section className="route-doctor-section" aria-labelledby="route-probes-title">
        <div className="route-doctor-section-head">
          <div>
            <p className="route-doctor-kicker">Direct probes</p>
            <h3 id="route-probes-title">中转站直连</h3>
          </div>
          <span className="route-doctor-count">
            {result ? `${result.relayProbes.length} 个` : "等待中"}
          </span>
        </div>
        {result?.relayProbes.length === 0 ? (
          <p className="empty-hint">没有可安全探测的 apikey 中转账号。</p>
        ) : null}
        <div className="route-doctor-probes">
          {result?.relayProbes.map((probe) => (
            <article key={probe.accountId}>
              <div className="route-doctor-probe-name">
                <strong>{probe.accountName}</strong>
                <span className="mono">#{probe.accountId} · {probe.upstreamHost}</span>
              </div>
              <div className="route-doctor-probe-checks">
                <div>
                  <span>/v1/models</span>
                  <strong className={probe.models.success ? "is-ok-text" : "is-danger-text"}>
                    {probe.models.statusCode ?? (probe.models.attempted ? "ERR" : "—")}
                  </strong>
                  <small>{probe.models.detail}</small>
                </div>
                <div>
                  <span>/v1/responses</span>
                  <strong
                    className={
                      !probe.responses.attempted
                        ? ""
                        : probe.responses.success
                          ? "is-ok-text"
                          : "is-danger-text"
                    }
                  >
                    {probe.responses.statusCode ?? (probe.responses.attempted ? "ERR" : "未运行")}
                  </strong>
                  <small>{probe.responses.detail}</small>
                </div>
              </div>
            </article>
          ))}
        </div>
      </section>
    </CardShell>
  );
}
