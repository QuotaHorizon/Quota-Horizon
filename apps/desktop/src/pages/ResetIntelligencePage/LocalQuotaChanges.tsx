import { useEffect, useState } from "react";
import type { DesktopHistoryEnvelope, DesktopQuotaRiseObservation, DesktopStatusEnvelope } from "../../../../capacity-preview/src/status";
import { getCapacityHistory, isDesktopApp } from "../../api/backend";
import type { Language } from "../../i18n";
import { quotaPercentLabel } from "../../utils/quotaPercent";
import { settleHistory, visibleHistory, type HistoryView } from "../CapacityPage/historyState";
import { historyEmptyMessage } from "../CapacityPage/presentation";
import { localResetFacts, publicEvidenceTime } from "./presentation";
import styles from "./index.module.less";

export function quotaRiseLabel(item: DesktopQuotaRiseObservation, language: Language) {
  const labels: Record<string, [string, string]> = {
    awaiting_followup: ["回升待复核", "Rise awaiting follow-up"],
    around_scheduled_boundary: ["窗口到期附近回升", "Rise around scheduled expiry"],
    before_scheduled_boundary: ["原窗口到期前回升", "Rise before scheduled expiry"],
    unverified: ["回升证据不足", "Rise with incomplete evidence"],
  };
  return (labels[item.classification] ?? labels.unverified)[language === "zh" ? 0 : 1];
}

export function quotaRiseExplanation(item: DesktopQuotaRiseObservation, language: Language) {
  const zh = language === "zh";
  if (item.limitation) {
    const reasons: Record<string, [string, string]> = {
      awaiting_followup: ["还需要至少 30 秒后的另一条有效观测，确认回升仍存在。", "Another valid capture at least 30 seconds later is needed to check that the rise persists."],
      observation_gap: ["前后观测间隔超过一小时，不能确认变化过程。", "An observation gap exceeds one hour, so the transition cannot be verified."],
      window_unavailable: ["缺少有效窗口或重置时间，不能核对周期变化。", "A window or reset time is missing; the cycle change cannot be checked."],
      window_not_reanchored: ["额度回升，重置时间未明显延后。", "Quota rose without a material delay in the reset time."],
      window_changed_again: ["复核前窗口再次变化，原来的判断不再充分。", "The window changed again before verification; the earlier evidence is insufficient."],
      not_sustained: ["后续额度回落，未满足持续回升条件。", "A later capture fell back and did not meet the sustained-rise condition."],
      ambiguous_timing: ["采样时间存在冲突，等待后续记录。", "Capture times conflict. Awaiting further records."],
      source_changed: ["复核期间额度桶或窗口时长变化，不能直接衔接。", "The quota bucket or window duration changed during follow-up."],
      invalid_sample: ["部分采样无效，等待后续记录。", "Some captures are invalid. Awaiting further records."],
    };
    return (reasons[item.limitation] ?? ["采样不足，等待后续记录。", "Not enough captures. Awaiting further records."])[zh ? 0 : 1];
  }
  if (!["around_scheduled_boundary", "before_scheduled_boundary"].includes(item.classification)) {
    return zh ? "等待后续采样确认回升。" : "Awaiting further captures to confirm the rise.";
  }
  return item.classification === "around_scheduled_boundary"
    ? (zh ? "原定到期前后额度持续回升，符合正常周期更新的时间。" : "The sustained rise spans the scheduled reset time, consistent with a normal rollover.")
    : (zh ? "原定到期前额度回升，重置时间延后；原因待确认。" : "Quota rose before expiry and the reset time moved later. Cause unconfirmed.");
}

export function LocalQuotaChangesView({ history, language, issue = false }: {
  history: DesktopHistoryEnvelope | null; language: Language; issue?: boolean;
}) {
  const zh = language === "zh";
  const [expanded, setExpanded] = useState(false);
  const summary = history?.status === "available" ? history.overview?.quotaChanges : undefined;
  const observations = summary?.observations.slice().reverse() ?? [];
  return <div className={styles.observation}>
    <strong>{zh ? "本机额度回升记录" : "Locally observed quota rises"}</strong>
    {!summary ? <p role="status">{history?.status === "available"
      ? (zh ? "暂时没有可用的回升分析。" : "Quota-rise analysis is not available yet.")
      : historyEmptyMessage(history, language)}
      {["history_keychain_denied", "history_keychain_interaction_required"].includes(history?.reasonCode ?? "")
        && (zh ? " 请前往「额度规划」授权。" : " Authorize history access on Capacity Planning.")}</p> : <>
      <p>{zh ? `最近历史中有 ${summary.validSamples} 条有效观测、${summary.comparableIntervals} 段相邻比较；${summary.excludedIntervals} 段因缺测或数据变化排除。`
        : `${summary.validSamples} valid captures and ${summary.comparableIntervals} adjacent comparisons; ${summary.excludedIntervals} intervals excluded for gaps or changed data.`}</p>
      {!!history?.points.length && <p>{zh ? "样本范围：" : "Capture range: "}{publicEvidenceTime(history.points[0].capturedAt, language)} → {publicEvidenceTime(history.points.at(-1)?.capturedAt, language)}</p>}
      {issue && <p role="status">{zh ? "更新失败，显示上次分析。" : "Update failed. Showing the previous analysis."}</p>}
      {observations.length === 0 && <p>{summary.validSamples < 3
        ? (zh ? "采样不足，正在积累记录。" : "Not enough captures yet. Recording continues.")
        : (zh ? "当前记录中没有达到 5 个百分点的回升。" : "No rise of at least 5 percentage points in these records.")}</p>}
      {summary.dateOnlyChanges > 0 && <p>{zh ? `${summary.dateOnlyChanges} 次窗口日期变化未伴随至少 5 个百分点的回升，没有按重置处理。`
        : `${summary.dateOnlyChanges} window changes had no rise of at least 5 percentage points and were not treated as resets.`}</p>}
      <div className={styles.timeline}>{observations.slice(0, expanded ? 32 : 3).map((item) => <article className={styles.evidenceCard}
        key={`${item.beforeSnapshotId}:${item.firstAfterSnapshotId}`}>
        <div className={styles.evidenceHeading}><h3>{quotaPercentLabel(item.beforeRemainingPercent)} → {quotaPercentLabel(item.afterRemainingPercent)}</h3>
          <span className={styles.badge}>{quotaRiseLabel(item, language)}</span></div>
        <p>{publicEvidenceTime(item.beforeAt, language)} → {publicEvidenceTime(item.firstAfterAt, language)}</p>
        <p>{quotaRiseExplanation(item, language)}</p>
        {item.confirmedAt && <p>{zh ? "后续观测：" : "Follow-up: "}{publicEvidenceTime(item.confirmedAt, language)} · {quotaPercentLabel(item.confirmedRemainingPercent)}</p>}
        <details className={styles.provenance}><summary>{zh ? "查看前后窗口" : "Compare window boundaries"}</summary>
          <p>{zh ? "原定重置：" : "Previous expiry: "}{publicEvidenceTime(item.beforeResetsAt, language)}</p>
          <p>{zh ? "回升后窗口：" : "Window after rise: "}{publicEvidenceTime(item.afterResetsAt, language)}</p>
        </details>
      </article>)}</div>
      {observations.length > 3 && <button className={styles.refreshButton} onClick={() => setExpanded((value) => !value)}>
        {expanded ? (zh ? "收起较早记录" : "Show fewer") : (zh ? `显示更多回升（还有 ${observations.length - 3} 条）` : `Show ${observations.length - 3} more rises`)}</button>}
      {summary.totalRises > observations.length && <p>{zh ? `范围内共有 ${summary.totalRises} 次回升，显示最近 ${observations.length} 次。` : `${summary.totalRises} rises in this history range; the latest ${observations.length} are retained in this view.`}</p>}
    </>}
  </div>;
}

/** Reuses the chart's context/late-response guards and the shared quota service;
 * no separate network request, timer, keychain prompt or account write. */
export function LocalQuotaChanges({ envelope, language }: { envelope: DesktopStatusEnvelope | null; language: Language }) {
  const limitId = localResetFacts(envelope).weekly?.limitId;
  const contextId = envelope?.historyContextId;
  const sequence = envelope?.sequence ?? 0;
  const key = JSON.stringify([contextId ?? `legacy-${sequence}`, envelope?.selectedExecutableId, limitId]);
  const [view, setView] = useState<HistoryView>({ key, history: null, issue: null });
  const current = visibleHistory(view, key);
  useEffect(() => {
    if (!isDesktopApp || !limitId || !sequence) return;
    let active = true;
    void getCapacityHistory(limitId).then((next) => {
      if (active) setView((previous) => settleHistory(previous, key, contextId, next));
    }).catch(() => {
      if (active) setView((previous) => settleHistory(previous, key, contextId, {
        schemaVersion: "1.0", historyContextId: contextId, status: "failed", reasonCode: "history_query_failed", points: [],
      }));
    });
    return () => { active = false; };
  }, [contextId, key, limitId, sequence]);
  return <LocalQuotaChangesView key={key} history={limitId ? current.history : {
    schemaVersion: "1.0", status: "unavailable", reasonCode: "history_context_unavailable", points: [],
  }} language={language} issue={!!current.issue} />;
}
