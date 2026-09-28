import type { CapacityDemandInput, DesktopDemandPlanEnvelope } from "../../../../capacity-preview/src/status";
import type { Language } from "../../i18n";

export function acceptDemandPlan(previous: DesktopDemandPlanEnvelope | null, next: DesktopDemandPlanEnvelope, context: string): DesktopDemandPlanEnvelope | null {
  if (next.historyContextId !== context) return null;
  if (next.status === "unavailable") return next;
  if (previous?.historyContextId === context && previous.plans[0]?.revision > (next.plans[0]?.revision ?? 0)) return previous;
  if (previous?.historyContextId === context && previous.status !== "unavailable"
    && previous.plans[0]?.revision === next.plans[0]?.revision
    && Date.parse(previous.generatedAt) > Date.parse(next.generatedAt)) return previous;
  return next;
}

export function localPlanInput(utc: string): string {
  const date = new Date(utc);
  if (!Number.isFinite(date.getTime())) return "";
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

export function demandFromForm(deadline: string, kind: CapacityDemandInput["demandKind"], hours: string, now = Date.now(), originalUtc?: string): CapacityDemandInput | null {
  if (!/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}$/u.test(deadline)) return null;
  const unchanged = originalUtc && Number.isFinite(Date.parse(originalUtc)) && localPlanInput(originalUtc) === deadline;
  const end = new Date(unchanged ? originalUtc : deadline);
  // Reject DST gaps/invalid calendar dates instead of silently moving the plan.
  if (!Number.isFinite(end.getTime()) || localPlanInput(end.toISOString()) !== deadline) return null;
  if (!unchanged) {
    // A manually chosen DST fold has two possible UTC instants. Require an
    // unambiguous time instead of silently changing a copied plan's instant.
    for (const minutes of [30, 60, 90, 120]) {
      for (const sign of [-1, 1]) if (localPlanInput(new Date(end.getTime() + sign * minutes * 60_000).toISOString()) === deadline) return null;
    }
  }
  const span = end.getTime() - now;
  const activeHours = kind === "active_hours" ? Number(hours) : null;
  if (span <= 0 || span > 366 * 86_400_000) return null;
  if (kind === "active_hours" && (!hours.trim() || activeHours == null || !Number.isFinite(activeHours) || activeHours <= 0 || activeHours * 3_600_000 > span)) return null;
  return { horizonEnd: end.toISOString(), demandKind: kind, plannedCodexActiveHours: activeHours };
}

export function planTime(value: string | null | undefined, language: Language): string {
  return value && Number.isFinite(Date.parse(value)) ? new Intl.DateTimeFormat(language === "zh" ? "zh-CN" : "en-US", {
    year: "numeric", month: "short", day: "numeric", hour: "2-digit", minute: "2-digit", hourCycle: "h23", timeZoneName: "short",
  }).format(new Date(value)) : "—";
}

const MESSAGES: Record<string, [string, string]> = {
  plan_context_changed: ["当前账号或读取环境已变化，正在等待新状态。不会将旧计划应用到新账号。", "The account or reader environment changed. Waiting for its state; the previous plan will not be applied."],
  plan_binding_required: ["先完成当前账号的本地历史绑定，才能独立保存计划。", "Bind local history for the current account before saving its plan."],
  plan_store_unavailable: ["本地计划暂不可用，请稍后重试；原数据没有删除。", "Local plans are unavailable. Retry later; existing data has not been deleted."],
  plan_revision_conflict: ["另一处已修改计划。请载入最新版本后再编辑，当前输入尚未保存。", "This plan changed elsewhere. Load the latest version before editing; your input has not been saved."],
  plan_input_invalid: ["请填写未来 366 天内、不处于夏令时歧义时段的截止时间；计划使用时长须大于 0，且不能超过剩余日历时长。", "Choose an unambiguous local deadline in the next 366 days. Planned active hours must be positive and fit before it."],
  plan_save_failed: ["保存失败，输入已保留，请重试。", "Save failed. Your input is retained; please retry."],
  plan_read_failed: ["计划更新失败，暂保留上次内容；预算暂不显示。", "Could not update the plan. Previous content is retained; allowances are hidden."],
  plan_ended: ["已到计划截止时间，请调整需求。", "The deadline has passed. Update your demand."],
  reset_before_deadline: ["此窗口会先于计划结束而重置，不将下一周期额度提前算入。", "This window resets before the deadline. Its next balance is not counted in advance."],
  window_ended: ["窗口已到重置时间，等待新的额度。", "This window has reached its reset time. Waiting for a new balance."],
  reset_unknown: ["尚无确切重置时间，暂不分配此窗口额度。", "The reset time is unknown; no allowance is calculated for this window."],
};

export function demandMessage(code: string, language: Language): string {
  if (code.startsWith("history_keychain")) return language === "zh" ? "本地历史密钥尚未授权，旧计划已保留。请先在额度历史中授权。" : "Local history access is not authorized. Existing plans are retained; authorize in Capacity history first.";
  return (MESSAGES[code] ?? ["正在等待可用的当前账号额度，暂不计算预算。", "Waiting for usable quota for the current account; no allowance is calculated."])[language === "zh" ? 0 : 1];
}
