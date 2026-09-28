import type { Language } from "../../i18n";
import { broadGrantDeliveryExcerpt } from "./resetDelivery";
import { isPublicWithdrawal, sameResetSubject } from "./publicCorrections";
import type { PublicResetArchiveEntry, PublicTimelineEntry } from "./types";

export type CalendarKind = "reset" | "grant" | "notice";
export type CalendarMode = "events" | "announcements";
type TimeBasis = "occurred" | "tracker_confirmation" | "reported_effective" | "grant_announcement" | "published" | "unknown";
export interface ResetCalendarRecord {
  key: string;
  kind: CalendarKind;
  mode: CalendarMode;
  reviewed: boolean;
  time: number | null;
  timeBasis: TimeBasis;
  title: string;
  summary: string;
  url: string;
  publishedAt: string | null;
  reportedAt: string | null;
  sources: { url: string; time: number | null; basis: TimeBasis }[];
  conflictingTimes: boolean;
  withdrawn: boolean;
}

type Input = { entry: PublicResetArchiveEntry | PublicTimelineEntry; archive: boolean };

/** A date or a zone-less time is not an instant. Never assume midnight, UTC or
 * the local zone to turn incomplete public evidence into a precise event. */
export function exactPublicTime(value: string | null | undefined): number | null {
  if (!value || !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})$/u.test(value)) return null;
  const [year, month, day] = value.slice(0, 10).split("-").map(Number);
  const monthDays = [31, year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0) ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
  if (month < 1 || month > 12 || day < 1 || day > monthDays[month - 1]) return null;
  const time = Date.parse(value);
  return Number.isFinite(time) ? time : null;
}

export function localCalendarDate(date: Date) {
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
}

export function resetCalendarMonths(now: number) {
  if (!Number.isFinite(now)) return [];
  const today = new Date(now);
  return [-2, -1, 0].map((offset) => {
    // Calendar arithmetic, not fixed 24h durations, across DST and year rollover.
    const first = new Date(today.getFullYear(), today.getMonth() + offset, 1, 12);
    const count = new Date(first.getFullYear(), first.getMonth() + 1, 0, 12).getDate();
    const padding = (first.getDay() + 6) % 7;
    return {
      key: localCalendarDate(first).slice(0, 7), first,
      days: Array.from({ length: 42 }, (_, index) => {
        const day = index - padding + 1;
        if (day < 1 || day > count) return null;
        const date = new Date(first.getFullYear(), first.getMonth(), day, 12);
        return { key: localCalendarDate(date), day, future: localCalendarDate(date) > localCalendarDate(today) };
      }),
    };
  });
}

function latestInputs(inputs: Input[]) {
  const latest = new Map<string, Input>();
  for (const input of inputs) {
    const signal = input.entry.signal;
    const key = `${input.archive ? "archive" : "live"}:${signal.signalId}`;
    const previous = latest.get(key);
    if (!previous || signal.revision > previous.entry.signal.revision) latest.set(key, input);
  }
  return [...latest.values()];
}

function record(input: Input, language: Language, now: number): ResetCalendarRecord | null {
  const { entry, archive } = input;
  const { signal, disposition } = entry;
  if (!["global_full_reset", "global_banked_reset_grant", "unclassified"].includes(signal.kind)
    || signal.scope === "personal" || signal.source.sourceClass === "provider_ui_observation"
    || disposition === "negative" || signal.semantics === "negative_signal") return null;
  const reviewed = archive && signal.source.review === "primary_reviewed" && signal.scope === "broad_codex"
    && ["official_announcement", "official_documentation", "official_social", "official_status"].includes(signal.source.sourceClass)
    && disposition === "confirmed";
  const withdrawn = isPublicWithdrawal(entry);
  const occurred = exactPublicTime(signal.occurredAt);
  const published = exactPublicTime(signal.source.publishedAt);
  const reportsOccurrence = signal.semantics === "confirmed_reset" || (reviewed && !signal.semantics);
  const occurrence = signal.kind !== "unclassified" && reportsOccurrence && occurred != null && occurred <= now && !withdrawn;
  const delivery = !withdrawn && published != null && published <= now
    && "summary" in signal && typeof signal.summary === "string" && broadGrantDeliveryExcerpt(signal, signal.summary) != null;
  const fromQuotaResets = signal.source.discoveredVia.some((url) => {
    try { return new URL(url).hostname === "quotaresets.com"; } catch { return false; }
  });
  const time = withdrawn ? null : occurrence ? occurred : published != null && published <= now ? published : null;
  const title = "title" in entry ? entry.title[language] : entry.signal.title;
  const summary = "summary" in entry ? entry.summary[language] : entry.signal.summary;
  const timeBasis: TimeBasis = time == null ? "unknown" : occurrence
    ? reviewed ? "occurred" : fromQuotaResets ? "tracker_confirmation" : "reported_effective" : delivery ? "grant_announcement" : "published";
  return {
    key: `${archive ? "archive" : "live"}:${signal.signalId}`,
    // A topic label is not an occurrence. Also repairs cached pre-v4 signals
    // without deleting their original provenance or historical revisions.
    kind: signal.kind === "global_banked_reset_grant" ? "grant" : occurrence && signal.kind === "global_full_reset" ? "reset" : "notice",
    // A dated, explicit rollout announcement belongs with grant records. Its
    // timestamp remains a publication time, never an invented receipt time.
    mode: occurrence || delivery ? "events" : "announcements", reviewed: occurrence && reviewed,
    time, timeBasis, title, summary, url: signal.source.canonicalUrl,
    publishedAt: signal.source.publishedAt, reportedAt: signal.occurredAt,
    sources: [{ url: signal.source.canonicalUrl, time, basis: timeBasis }],
    conflictingTimes: false, withdrawn,
  };
}

/** Group mirror families and explicit event identities transitively, without
 * inferring identity from similar text or nearby dates. A reviewed document
 * without an occurrence time cannot lend its authority to a tracker's time. */
export function resetCalendarRecords(archive: PublicResetArchiveEntry[], live: PublicTimelineEntry[], language: Language, now: number) {
  const inputs = latestInputs([...archive.map((entry) => ({ entry, archive: true })), ...live.map((entry) => ({ entry, archive: false }))]);
  // A denial is not a calendar event, but must still suppress a mirrored old
  // occurrence. Do this before filtering negatives out of calendar records.
  const withdrawals = inputs.filter((input) => isPublicWithdrawal(input.entry));
  const familyKinds = new Map<string, Set<string>>();
  for (const { entry: { signal } } of inputs) {
    if (signal.kind === "unclassified") continue;
    const kinds = familyKinds.get(signal.evidenceFamilyId) ?? new Set<string>();
    kinds.add(signal.kind); familyKinds.set(signal.evidenceFamilyId, kinds);
  }
  const groups: { input: Input; value: ResetCalendarRecord }[][] = [];
  const aliases = new Map<string, number>();
  const roots: number[] = [];
  const root = (id: number): number => {
    while (roots[id] !== id) { roots[id] = roots[roots[id]]; id = roots[id]; }
    return id;
  };
  for (const input of inputs) {
    const value = record(input, language, now);
    if (!value) continue;
    const { signal } = input.entry;
    // Group by topic identity, not current visual status, so a withdrawal still
    // suppresses an older occurrence mirrored by another source.
    const knownKinds = familyKinds.get(signal.evidenceFamilyId);
    const groupKind = signal.kind === "unclassified" && knownKinds?.size === 1 ? [...knownKinds][0] : signal.kind;
    const keys = [`family:${groupKind}:${signal.evidenceFamilyId}`];
    if (signal.eventId) keys.push(`event:${groupKind}:${signal.eventId}`);
    const matches = [...new Set(keys.flatMap((key) => aliases.has(key) ? [root(aliases.get(key)!)] : []))];
    const id = matches[0] ?? groups.length;
    if (id === groups.length) { groups.push([]); roots.push(id); }
    for (const other of matches.slice(1)) { roots[other] = id; groups[id].push(...groups[other]); groups[other] = []; }
    groups[id].push({ input, value });
    keys.forEach((key) => aliases.set(key, id));
  }
  const rank = (item: ResetCalendarRecord) => (item.mode === "events" ? item.timeBasis === "grant_announcement" ? 8 : 10 : 0)
    + (item.reviewed ? 5 : 0) + (item.time != null ? 1 : 0);
  return groups.filter((group) => group.length).map((group) => {
    group.sort((a, b) => rank(b.value) - rank(a.value)
      || (exactPublicTime(b.input.entry.signal.recordedAt) ?? 0) - (exactPublicTime(a.input.entry.signal.recordedAt) ?? 0)
      || a.value.key.localeCompare(b.value.key));
    const selected = group[0].value;
    const withdrawn = group.some((item) => item.value.withdrawn
      || withdrawals.some((other) => sameResetSubject(item.input.entry.signal, other.entry.signal)));
    const sources = group.flatMap((item) => item.value.sources).filter((source, index, all) => all.findIndex((other) => other.url === source.url && other.time === source.time && other.basis === source.basis) === index);
    // Announcement and delivery/confirmation can legitimately have different
    // times. Only competing occurrence times constitute a time conflict.
    const occurrenceTimes = new Set(group.filter((item) => item.value.mode === "events"
      && item.value.timeBasis !== "grant_announcement").map((item) => item.value.time));
    const announcementTimes = new Set(group.filter((item) => item.value.timeBasis === "grant_announcement").map((item) => item.value.time));
    const conflictingTimes = occurrenceTimes.size > 1 || (occurrenceTimes.size === 0 && announcementTimes.size > 1);
    return { ...selected, key: group.map((item) => item.value.key).sort()[0],
      sources, withdrawn, conflictingTimes,
      // A correction/withdrawal from a mirror is a review task, not an event.
      ...(withdrawn || conflictingTimes ? { time: null, reviewed: false } : {}),
    };
  }).sort((a, b) => (b.time ?? -Infinity) - (a.time ?? -Infinity) || a.key.localeCompare(b.key));
}

export function calendarEventTime(time: number | null, language: Language) {
  if (time == null || !Number.isFinite(time)) return "—";
  return new Intl.DateTimeFormat(language === "zh" ? "zh-CN" : "en-US", {
    year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", second: "2-digit",
    hourCycle: "h23", timeZoneName: "longOffset",
  }).format(time);
}

export function calendarTimeLabel(basis: TimeBasis, language: Language) {
  const labels: Record<TimeBasis, [string, string]> = {
    occurred: ["发生时间", "Occurrence time"],
    tracker_confirmation: ["追踪站报告时间", "Tracker report time"],
    reported_effective: ["来源标注的生效时间", "Source-reported effective time"],
    grant_announcement: ["发放公告时间", "Rollout announcement time"],
    published: ["消息发布时间", "Publication time"],
    unknown: ["缺少可换算的确切时间", "No exact time available for conversion"],
  };
  return labels[basis][language === "zh" ? 0 : 1];
}
