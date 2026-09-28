import type { Language } from "../../i18n";
import { exactPublicTime } from "./resetCalendarModel";
import { broadGrantDeliveryExcerpt } from "./resetDelivery";
import { isPublicWithdrawal, sameResetSubject } from "./publicCorrections";
import type { PublicResetTimeline, PublicRevisionChange, PublicTimelineEntry } from "./types";

type Signal = PublicRevisionChange["current"];
type Stage = "lead" | "announced" | "scheduled" | "rollout" | "reported" | "corrected" | "retracted" | "negative" | "excluded" | "context";
type ChangeKind = "first" | "progress" | "kind" | "timing" | "text";

export interface ResetSourceChange {
  key: string;
  current: Signal;
  previous: Signal | null;
  stage: Stage;
  previousStage: Stage | null;
  kind: ChangeKind;
  recordedAt: number;
  backfilled: boolean;
  withdrawn: boolean;
  sources: string[];
  unread: boolean;
}

const isReset = (signal: Signal) => ["global_full_reset", "global_banked_reset_grant"].includes(signal.kind);
const words = (value: string) => value.replace(/\s+/gu, " ").trim();

function relevant(signal: Signal) {
  return signal.scope !== "personal" && signal.source.sourceClass !== "provider_ui_observation"
    && (isReset(signal) || (signal.kind === "unclassified" && signal.semantics != null && signal.semantics !== "context_only"));
}

function stageOf(signal: Signal): Stage {
  if (signal.scope === "personal" || signal.source.sourceClass === "provider_ui_observation") return "excluded";
  if (signal.semantics === "corrected" || signal.semantics === "retracted") return signal.semantics;
  if (signal.semantics === "negative_signal") return "negative";
  if (!isReset(signal) && signal.kind !== "unclassified") return "context";
  if (signal.semantics === "confirmed_reset" && isReset(signal)) return "reported";
  if (broadGrantDeliveryExcerpt(signal, signal.summary)) return "rollout";
  if (signal.semantics === "explicit_timed_reset") return "scheduled";
  if (signal.semantics === "explicit_future_reset") return "announced";
  if (signal.semantics === "possible_signal") return "lead";
  return "context";
}

function timing(signal: Signal) {
  return [exactPublicTime(signal.occurredAt), signal.announcementTiming?.expectedOn ?? null,
    signal.announcementTiming?.timeZone ?? null, exactPublicTime(signal.source.publishedAt)];
}

function changeKind(previous: Signal | null, current: Signal): ChangeKind | null {
  if (!previous) return "first";
  if (stageOf(previous) !== stageOf(current)) return "progress";
  if (previous.kind !== current.kind) return "kind";
  if (JSON.stringify(timing(previous)) !== JSON.stringify(timing(current))) return "timing";
  if (words(previous.summary) !== words(current.summary) || words(previous.title) !== words(current.title)
    || previous.source.canonicalUrl !== current.source.canonicalUrl) return "text";
  return null;
}

/** Reconstruct actual adjacent revisions, not imagined progress from a latest
 * snapshot. First collection is always labelled discovery, never occurrence.
 * Mirrors of the same original and same transition collapse to one change. */
export function resetSourceChanges(timeline: PublicResetTimeline | null, now: number): ResetSourceChange[] {
  if (!timeline?.changes || !Number.isFinite(now)) return [];
  const latest = new Map<string, PublicTimelineEntry>();
  for (const entry of timeline.entries) {
    const { signal } = entry;
    const time = exactPublicTime(signal.recordedAt);
    if (time == null || time > now) continue;
    if (!latest.has(signal.signalId) || latest.get(signal.signalId)!.signal.revision < signal.revision) latest.set(signal.signalId, entry);
  }
  const withdrawals = [...latest.values()].filter((entry) => isPublicWithdrawal(entry) || stageOf(entry.signal) === "excluded");
  const changes = new Map<string, ResetSourceChange>();
  // Process earliest discovery first so a later mirror does not make old news
  // appear fresh. A genuinely repeated transition within one signal is retained.
  const ordered = [...timeline.changes.items].sort((a, b) => (exactPublicTime(a.current.recordedAt) ?? Infinity)
    - (exactPublicTime(b.current.recordedAt) ?? Infinity) || a.current.signalId.localeCompare(b.current.signalId) || a.current.revision - b.current.revision);
  for (const { previous, current, unread } of ordered) {
    const recordedAt = exactPublicTime(current.recordedAt);
    if (recordedAt == null || recordedAt > now || (!relevant(current) && !(previous && relevant(previous)))) continue;
    if (previous && (previous.signalId !== current.signalId || previous.evidenceFamilyId !== current.evidenceFamilyId
      || previous.revision + 1 !== current.revision || exactPublicTime(previous.recordedAt) == null
      || exactPublicTime(previous.recordedAt)! > recordedAt)) continue;
    if (!previous && current.revision !== 1) continue;
    const kind = changeKind(previous, current);
    if (!kind) continue;
    const stage = stageOf(current); const previousStage = previous ? stageOf(previous) : null;
    const signature = (signal: Signal | null) => signal ? [signal.kind, stageOf(signal), ...timing(signal)] : null;
    const mirrorKey = JSON.stringify([current.evidenceFamilyId, kind, signature(previous), signature(current),
      kind === "text" ? [words(previous?.summary ?? ""), words(current.summary)] : null]);
    const mirror = changes.get(mirrorKey);
    if (mirror && mirror.current.signalId !== current.signalId) {
      mirror.sources = [...new Set([...mirror.sources, ...current.source.discoveredVia])];
      continue;
    }
    const published = exactPublicTime(current.source.publishedAt);
    const item: ResetSourceChange = {
      key: `${current.signalId}:${current.revision}`, current, previous, kind, stage, previousStage, recordedAt,
      backfilled: !previous && published != null && recordedAt - published >= 24 * 3_600_000,
      withdrawn: withdrawals.some((entry) => sameResetSubject(entry.signal, current)),
      sources: [...new Set(current.source.discoveredVia)],
      unread: unread === true,
    };
    // Preserve repeated state cycles; only cross-source mirrors are duplicates.
    if (mirror) changes.set(`${mirrorKey}:${mirror.key}`, mirror);
    changes.set(mirrorKey, item);
  }
  return [...changes.values()].sort((a, b) => b.recordedAt - a.recordedAt || b.current.revision - a.current.revision || a.key.localeCompare(b.key));
}

/** A source wording edit or historical backfill is still readable in the
 * timeline, but must not repeatedly light up the everyday entry points. */
export function isNewResetProgress(change: ResetSourceChange) {
  if (!change.unread || change.backfilled || !["first", "progress"].includes(change.kind)) return false;
  if (["announced", "scheduled", "rollout", "reported", "corrected", "retracted", "negative"].includes(change.stage)) return true;
  // Ordinary tracker leads and metadata edits stay in Details without a badge.
  return change.kind === "first" && change.stage === "lead"
    && /^https:\/\/(?:www\.)?(?:x\.com|twitter\.com)\/thsottiaux\/status\/\d+/iu.test(change.current.source.canonicalUrl);
}

export function unreadResetProgress(timeline: PublicResetTimeline | null, now: number) {
  return resetSourceChanges(timeline, now).filter(isNewResetProgress);
}

export function resetStageLabel(stage: Stage, language: Language) {
  const labels: Record<Stage, [string, string]> = {
    lead: ["重置线索", "Reset lead"], announced: ["重置预告", "Announcement"], scheduled: ["带时间的预告", "Timed announcement"],
    rollout: ["宣布开始发卡", "Card rollout announced"], reported: ["追踪站报告已发生", "Tracker reports occurrence"],
    corrected: ["追踪记录已更正", "Tracker record corrected"], retracted: ["追踪记录已撤回", "Tracker record retracted"], negative: ["追踪记录标为否定", "Tracker marks a denial"],
    excluded: ["仅限个人观测", "Personal observation only"], context: ["背景讨论", "Context only"],
  };
  return labels[stage][language === "zh" ? 0 : 1];
}

export function resetChangeLabel(change: ResetSourceChange, language: Language) {
  const labels: Record<ChangeKind, [string, string]> = {
    first: change.backfilled ? ["历史补录", "Historical record added"] : ["首次收录", "First collected"],
    progress: ["状态变化", "Status changed"], kind: ["事件类型调整", "Event type changed"],
    timing: ["时间依据更新", "Timing updated"], text: ["收录内容更新", "Collected wording updated"],
  };
  return labels[change.kind][language === "zh" ? 0 : 1];
}

export function resetTopicLabel(signal: Signal, language: Language) {
  const labels = signal.kind === "global_banked_reset_grant" ? ["重置卡", "Reset cards"]
    : signal.kind === "global_full_reset" ? ["额度重置", "Quota reset"] : ["类型待确认", "Type unresolved"];
  return labels[language === "zh" ? 0 : 1];
}
