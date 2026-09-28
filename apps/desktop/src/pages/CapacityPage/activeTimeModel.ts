import type { ActiveTimeInput, ActiveTimer, DesktopActiveTimeEnvelope } from "../../../../capacity-preview/src/status";
import type { Language } from "../../i18n";

export function acceptActiveTime(current: DesktopActiveTimeEnvelope | null, next: DesktopActiveTimeEnvelope, context: string): DesktopActiveTimeEnvelope | null {
  if (next.historyContextId !== context) return null;
  if (next.status === "unavailable") return next; // Binding loss clears private presentation immediately.
  if (next.status === "failed" && current && current.status !== "unavailable" && next.revision < current.revision) return {...current,status:"failed",reasonCode:next.reasonCode};
  if (current && current.status !== "unavailable" && (next.revision < current.revision || (next.revision === current.revision && Date.parse(next.generatedAt) < Date.parse(current.generatedAt)))) return current;
  return next;
}
export function localActivityInput(utc: string): string {
  const d = new Date(utc); if (!Number.isFinite(d.getTime())) return "";
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth()+1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}
export function activityFromForm(start: string, end: string, minutes: string, now = Date.now(), exactDuration?: number, original?: ActiveTimeInput): ActiveTimeInput | null {
  const parseLocal = (input: string, originalUtc?: string) => {
    if (!/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}(:\d{2})?$/.test(input)) return NaN;
    const canonical = input.length === 16 ? `${input}:00` : input;
    if (originalUtc && canonical === localActivityInput(originalUtc)) return Date.parse(originalUtc);
    const date = new Date(canonical);
    // Reject nonexistent local times and ambiguous fall-back folds. A manual
    // observation must not silently select the wrong occurrence of an hour.
    if (localActivityInput(date.toISOString()) !== canonical) return NaN;
    for (const offset of [30,60,90,120]) {
      if (localActivityInput(new Date(date.getTime()+offset*60_000).toISOString()) === canonical) return NaN;
    }
    return date.getTime();
  };
  let from: number; let to: number;
  try { from = parseLocal(start, original?.startedAt); to = parseLocal(end, original?.endedAt); } catch { return null; }
  const seconds = exactDuration ?? Math.round(Number(minutes)*60);
  if (!minutes.trim() || !Number.isFinite(from) || !Number.isFinite(to) || to > now || to <= from || to-from > 7*86_400_000 || !Number.isInteger(seconds) || seconds < 1 || seconds > 86_400 || seconds*1000 > to-from) return null;
  return { startedAt: new Date(from).toISOString(), endedAt: new Date(to).toISOString(), durationSeconds: seconds };
}
export function suggestedTimerSeconds(timer: ActiveTimer, now: number): number {
  const extra = timer.state === "running" ? Math.max(0,Math.floor((now-Date.parse(timer.updatedAt))/1000)) : 0;
  return Math.min(86_400,timer.suggestedSeconds+extra);
}
export function timerDisplay(seconds: number): string {
  return [Math.floor(seconds/3600),Math.floor(seconds/60)%60,seconds%60].map((n)=>String(n).padStart(2,"0")).join(":");
}
export function activityMessage(code: string, language: Language): string {
  const zh = language === "zh";
  switch (code) {
    case "activity_overlap": return zh ? "这段时间与已纳入的记录重叠。请调整时间，或先排除重复记录。" : "This interval overlaps an included record. Adjust it or exclude the duplicate first.";
    case "activity_timer_open": return zh ? "先核对或放弃当前计时，再补录另一段使用。" : "Review or discard the current timer before recording another interval.";
    case "activity_revision_conflict": return zh ? "另一处已更新了计时或记录，请读取最新状态后再操作。" : "Another view updated this timer or record. Load its latest state before continuing.";
    case "activity_input_invalid": return zh ? "请检查开始、结束和实际分钟数：结束不能在未来，使用时间不能超过所选时段。" : "Check the dates and actual minutes: the end cannot be in the future and duration cannot exceed the interval.";
    case "activity_save_failed": return zh ? "保存未确认，输入已保留。请重试；相同记录不会重复保存。" : "Save was not confirmed. Your input is retained; retrying will not duplicate the same record.";
    case "activity_read_failed": case "activity_store_unavailable": return zh ? "暂时无法读取本地使用记录，请重试。" : "Local activity records are unavailable. Please retry.";
    default: return zh ? "等待当前账号的本地历史绑定。已有记录保留在本机，不会自动请求钥匙串授权。" : "Waiting for this account’s local history binding. Existing records stay on this device; no Keychain prompt is requested automatically.";
  }
}
