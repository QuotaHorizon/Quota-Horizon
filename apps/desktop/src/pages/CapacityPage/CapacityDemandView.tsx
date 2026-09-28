import { useState, type ReactNode } from "react";
import { ArrowUpRight, CalendarClock, Pencil, Target } from "lucide-react";
import type { DesktopDemandPlan, DesktopDemandPlanEnvelope, DesktopDemandPlanUpdate } from "../../../../capacity-preview/src/status";
import type { Language } from "../../i18n";
import { demandFromForm, demandMessage, localPlanInput, planTime } from "./demandPlanModel";
import styles from "./CapacityDemand.module.less";

export interface CapacityDemandViewProps {
  language: Language;
  contextId: string;
  value: DesktopDemandPlanEnvelope | null;
  failed?: boolean;
  busy?: boolean;
  compact?: boolean;
  onSave: (request: DesktopDemandPlanUpdate) => Promise<boolean>;
  onRetry: () => void;
  onOpen?: () => void;
  now?: number;
  renderArchive?: (onUse: (plan: DesktopDemandPlan) => void, canUse: boolean) => ReactNode;
}

export function CapacityDemandView({ language, contextId, value, failed, busy, compact, onSave, onRetry, onOpen, renderArchive, now = Date.now() }: CapacityDemandViewProps) {
  const zh = language === "zh";
  const [draft, setDraft] = useState<{ revision: number; deadline: string; originalUtc: string; kind: "active_hours" | "maintain_recent_pace"; hours: string; copied?: boolean } | null>(null);
  const [archiveOpen, setArchiveOpen] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const plan = value?.plans[0];
  const available = value && value.historyContextId === contextId && value.status !== "unavailable" && value.reasonCode !== "plan_read_failed";
  const expired = !!plan && Date.parse(plan.demand.horizonEnd) <= now;
  const changed = draft && draft.revision !== (plan?.revision ?? 0);
  const number = (n: number) => new Intl.NumberFormat(zh ? "zh-CN" : "en-US", { maximumFractionDigits: 1 }).format(n);
  const amount = (n: number) => n > 0 && n < 0.1 ? "<0.1" : number(n);
  const intentLabel = (item: DesktopDemandPlan) => item.demand.demandKind === "active_hours"
    ? (zh ? `计划主动使用 ${number(item.demand.plannedCodexActiveHours ?? 0)} 小时` : `${number(item.demand.plannedCodexActiveHours ?? 0)} planned active hours`)
    : (zh ? "维持近期使用节奏" : "Maintain my recent pace");
  const edit = () => {
    setError(null);
    const originalUtc = plan?.demand.horizonEnd ?? new Date(now + 86_400_000).toISOString();
    setDraft({ revision: plan?.revision ?? 0, deadline: localPlanInput(originalUtc), originalUtc, kind: plan?.demand.demandKind ?? "maintain_recent_pace", hours: plan?.demand.plannedCodexActiveHours?.toString() ?? "" });
  };
  const useArchivedPlan = (old: DesktopDemandPlan) => {
    if (draft || busy || failed || !available) return;
    setError(null);
    setDraft({revision: plan?.revision ?? 0, deadline: localPlanInput(old.demand.horizonEnd), originalUtc: old.demand.horizonEnd, kind: old.demand.demandKind, hours: old.demand.plannedCodexActiveHours?.toString() ?? "", copied: true});
    setArchiveOpen(false);
  };
  const save = async () => {
    if (!draft || busy || changed || failed || !available) return;
    const demand = demandFromForm(draft.deadline, draft.kind, draft.hours, Date.now(), draft.originalUtc);
    if (!demand) { setError("plan_input_invalid"); return; }
    setError(null);
    if (await onSave({ historyContextId: contextId, expectedRevision: draft.revision, enabled: true, demand })) {
      setDraft(null); setError(null);
    } else setError("plan_save_failed");
  };
  const title = zh ? "我的工作需求" : "My work demand";

  return <section className={`${styles.panel} ${compact ? styles.compact : ""}`} aria-label={title}>
    <div className={styles.heading}><span><Target size={compact ? 14 : 18} /><h3>{title}</h3></span>
      {compact ? <button type="button" onClick={onOpen}>{zh ? "设置" : "Edit"}<ArrowUpRight size={12} /></button>
        : available && !draft && <button type="button" disabled={busy || failed} onClick={edit}><Pencil size={13} />{plan ? (zh ? "编辑需求" : "Edit demand") : (zh ? "设置计划" : "Set a plan")}</button>}
    </div>
    {!value ? <p className={styles.muted}>{failed ? demandMessage("plan_read_failed", language) : zh ? "正在读取当前账号的计划…" : "Loading this account’s plan…"}</p>
      : !available ? <p className={styles.muted}>{demandMessage(value.historyContextId !== contextId ? "plan_context_changed" : value.reasonCode, language)}</p>
      : <>
        {plan ? <div className={styles.summary}>
          <strong>{intentLabel(plan)} <small>{!plan.enabled ? (zh ? "已暂停" : "Paused") : expired ? (zh ? "已到期" : "Ended") : (zh ? "进行中" : "Active")}</small></strong>
          <span><CalendarClock size={13} />{zh ? "截至 " : "Until "}{planTime(plan.demand.horizonEnd, language)}</span>
        </div> : <p className={styles.muted}>{zh ? "设置截止时间或计划使用时长。" : "Set a deadline or planned active hours."}</p>}
        {value.hasOtherEnvironmentPlans && <p className={styles.muted}>{zh ? "有旧版 Codex 的计划，可在下方「旧读取环境的计划与记录」查看。" : "Plans from earlier Codex versions are available under “Plans and records from earlier readers” below."}</p>}

        {!compact && draft && <form className={styles.form} onSubmit={(event) => { event.preventDefault(); void save(); }}>
          {draft.copied && <p className={styles.muted}>{zh ? "旧计划已载入草稿。请核对时长和截止时间，再保存为新计划。" : "Earlier values are loaded as a draft. Review the hours and deadline, then save as a new plan."}</p>}
          <label>{zh ? "希望使用到（电脑本地时区）" : "Use until (computer’s timezone)"}
            <input required autoFocus={draft.copied} type="datetime-local" value={draft.deadline} disabled={busy} onChange={(event) => setDraft({ ...draft, deadline: event.target.value })} />
            <small>{planTime(demandFromForm(draft.deadline, "maintain_recent_pace", "", now, draft.originalUtc)?.horizonEnd, language)}</small>
          </label>
          <label>{zh ? "需求口径" : "Demand basis"}<select value={draft.kind} disabled={busy} onChange={(event) => setDraft({ ...draft, kind: event.target.value as typeof draft.kind })}>
            <option value="maintain_recent_pace">{zh ? "维持近期使用节奏" : "Maintain my recent pace"}</option>
            <option value="active_hours">{zh ? "明确的主动使用小时数" : "Explicit active hours"}</option>
          </select></label>
          {draft.kind === "active_hours" && <label>{zh ? "计划主动使用 Codex（小时）" : "Planned active Codex use (hours)"}<input required type="number" min="0.1" step="0.1" value={draft.hours} disabled={busy} onChange={(event) => setDraft({ ...draft, hours: event.target.value })} />
            <small>{zh ? "填写计划使用 Codex 的小时数。" : "Enter the hours you plan to use Codex."}</small></label>}
          <div className={styles.actions}>
            <button type="submit" disabled={busy || failed || !!changed}>{busy ? (zh ? "正在保存…" : "Saving…") : (zh ? "保存计划" : "Save plan")}</button>
            <button type="button" disabled={busy} onClick={() => { setDraft(null); setError(null); }}>{zh ? "取消" : "Cancel"}</button>
            {changed && <button type="button" onClick={edit}>{zh ? "载入最新计划" : "Load latest plan"}</button>}
          </div>
        </form>}

        {!compact && plan?.enabled && !expired && <>
          <div className={styles.result}>
            <strong>{zh ? "均摊预算" : "Evenly allocated budget"}</strong>
            <p>{zh ? "按计划时间均摊当前余额。" : "Divides the current balance across the planned time."}</p>
            {plan.demand.demandKind === "active_hours" && <p>{zh ? "请根据实际完成情况调整剩余计划时长。" : "Update remaining planned hours as you complete work."}</p>}
            {failed || !value.allowances.length ? <p>{demandMessage("quota_unavailable", language)}</p> : value.allowances.map((window) => <div className={styles.allowance} key={window.limitId}>
              <span>{window.windowMinutes >= 1440 ? `${number(window.windowMinutes / 1440)}${zh ? " 天窗口" : "-day window"}` : `${number(window.windowMinutes / 60)}${zh ? " 小时窗口" : "-hour window"}`} · {zh ? "当前 " : "Now "}{amount(window.remainingPercent)}%</span>
              {window.allocation.dailyPercent == null ? <small>{demandMessage(window.allocation.reasonCode, language)} {window.resetsAt && planTime(window.resetsAt, language)}</small> : <>
                <strong>{amount(window.allocation.dailyPercent)}% <small>{Date.parse(plan.demand.horizonEnd) - now < 86_400_000 ? (zh ? " / 剩余计划时段" : " / remaining plan") : (zh ? " / 每 24 小时" : " / 24 hours")}</small></strong>
                {window.allocation.perPlannedHourPercent != null && (plan.demand.plannedCodexActiveHours ?? 0) >= 1 && <small>{zh ? "按全部计划时长分配当前余额：" : "Current balance across all planned hours: "}{amount(window.allocation.perPlannedHourPercent)}%{zh ? " / 活跃小时" : " / active hour"}</small>}
              </>}
            </div>)}
            <small>{zh ? "额度采集于 " : "Quota captured "}{planTime(value.observedAt, language)}{value.freshness !== "live" ? (zh ? " · 旧快照，等待更新" : " · cached, awaiting update") : ""}</small>
          </div>
        </>}
        {!compact && plan && !draft && <div className={styles.actions}>
          {plan.enabled && <button type="button" disabled={busy || failed} onClick={() => void onSave({ historyContextId: contextId, expectedRevision: plan.revision, enabled: false, demand: plan.demand })}>{zh ? "暂停此计划" : "Pause this plan"}</button>}
          <small>{zh ? `第 ${plan.revision} 版 · 编辑保留旧版本` : `Revision ${plan.revision} · previous versions retained`}</small>
        </div>}
        {!compact && value.plans.length > 1 && <details className={styles.revisions}><summary>{zh ? "最近的计划版本" : "Recent plan versions"}</summary>
          <ol>{value.plans.slice(1).map((item) => <li key={item.revision}><strong>{zh ? `第 ${item.revision} 版` : `Revision ${item.revision}`}</strong> · {intentLabel(item)} · {item.enabled ? (zh ? "启用" : "Enabled") : (zh ? "暂停" : "Paused")}<br />{zh ? "截至 " : "Until "}{planTime(item.demand.horizonEnd, language)}<small>{zh ? "保存于 " : "Saved "}{planTime(item.createdAt, language)}</small></li>)}</ol>
        </details>}
        {!compact && renderArchive && <div className={styles.revisions}><button type="button" aria-expanded={archiveOpen} onClick={() => setArchiveOpen(open => !open)}>{archiveOpen ? (zh ? "收起旧环境记录" : "Hide earlier-reader records") : (zh ? "旧读取环境的计划与记录" : "Plans and records from earlier readers")}</button>
          {archiveOpen && renderArchive(useArchivedPlan, !draft && !busy && !failed && !!available)}
        </div>}
      </>}
    {(failed || error || changed || value?.status === "revision_conflict" || value?.status === "invalid_request" || value?.status === "failed") && <div className={styles.error} role="status">
      {demandMessage(changed || value?.status === "revision_conflict" ? "plan_revision_conflict" : value?.status === "invalid_request" ? "plan_input_invalid" : error ?? (failed ? "plan_read_failed" : value?.reasonCode ?? "plan_read_failed"), language)}
      {failed && <button type="button" onClick={onRetry}>{zh ? "重试" : "Retry"}</button>}
    </div>}
  </section>;
}
