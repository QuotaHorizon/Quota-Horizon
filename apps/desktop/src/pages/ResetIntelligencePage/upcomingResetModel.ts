import { exactPublicTime } from "./resetCalendarModel";
import { isTiboStatement, resetDeliveryExcerpt } from "./resetDelivery";
import { isPublicWithdrawal, latestPublicEntries, sameResetSubject } from "./publicCorrections";
import type { PublicResetTimeline, PublicTimelineEntry } from "./types";

export { resetDeliveryExcerpt } from "./resetDelivery";

export interface UpcomingResetNotice {
  key: string;
  kind: "reset" | "grant" | "unknown";
  excerpt: string;
  url: string;
  publishedAt: number;
  collectedVia: string[];
  trackerCount: number;
  state: "upcoming" | "elapsed" | "reported" | "withdrawn" | "announced_delivery";
  timing: { start: number; end: number; timeZone: string; sourceUrl: string; deadline: boolean; explicit: boolean } | null;
  explicit: boolean;
  cohort: string | null;
  reportedAt?: number | null;
  stale: boolean;
  conflictingTiming: boolean;
}

export function activeResetCommitment(notice: UpcomingResetNotice | null | undefined, now: number, failed = false) {
  return !!notice && !failed && notice.explicit && notice.kind === "reset" && notice.state === "upcoming"
    && !notice.stale && !notice.conflictingTiming && notice.timing?.explicit === true
    && Number.isFinite(notice.timing.end) && notice.timing.end > now;
}

/** Calendar-date bounds in the source's named zone, not a hard-coded Pacific
 * offset. Refuse unresolvable midnight boundaries rather than fabricate them. */
export function sourceDateBounds(date: string, timeZone: string) {
  const base = exactPublicTime(`${date}T00:00:00Z`);
  if (base == null || timeZone.length > 64) return null;
  try {
    const format = new Intl.DateTimeFormat("en-US", { timeZone, year: "numeric", month: "2-digit", day: "2-digit",
      hour: "2-digit", minute: "2-digit", second: "2-digit", hourCycle: "h23" });
    const atMidnight = (target: number) => {
      let guess = target;
      for (let iteration = 0; iteration < 5; iteration++) {
        const parts = Object.fromEntries(format.formatToParts(guess).map((part) => [part.type, part.value]));
        const actual = Date.UTC(Number(parts.year), Number(parts.month) - 1, Number(parts.day), Number(parts.hour), Number(parts.minute), Number(parts.second));
        if (actual === target) return guess;
        guess += target - actual;
      }
      return null;
    };
    const start = atMidnight(base); const end = atMidnight(base + 86_400_000);
    return start != null && end != null && end > start ? { start, end } : null;
  } catch { return null; }
}

export function resetCommitmentExcerpt(text: string): string | null {
  const parts = text.split(/([.!?](?:\s+|$)|\n+)/u);
  // No lookbehind: retain compatibility with the desktop's older WebKit target.
  for (let index = 0; index < parts.length; index += 2) {
    const sentence = `${parts[index]}${parts[index + 1] ?? ""}`.trim();
    // This selects a visible excerpt of a reported commitment, not an official
    // confirmation, a global scope determination or a probability estimate.
    if (/\b(?:not|never|no|won't|wouldn't|might|may|could|hope|wish|if|unless|yesterday|already)\b/iu.test(sentence) || sentence.includes("?")) continue;
    if (/\b(?:will|going to)\b[^.!?]{0,200}\breset\b|\breset\b[^.!?]{0,200}\b(?:landing|lands|arriving|arrives|coming)\b|\b(?:I|we) (?:have )?promised? (?:a |the |another )?(?:usage |quota )?reset (?:for|on|by)\b|\b(?:the |a )?reset is scheduled (?:for|on)\b/iu.test(sentence)) {
      return sentence.trim().length <= 360 ? sentence.trim() : null;
    }
  }
  return null;
}

export function upcomingResetNotices(timeline: PublicResetTimeline | null, now: number): UpcomingResetNotice[] {
  if (!timeline || !Number.isFinite(now)) return [];
  const latest = latestPublicEntries(timeline.entries.filter(({ signal }) => {
    const recorded = exactPublicTime(signal.recordedAt);
    return recorded != null && recorded <= now;
  }));
  const groups = new Map<string, PublicTimelineEntry[]>();
  for (const entry of latest) {
    if (!isTiboStatement(entry.signal)) continue;
    const key = entry.signal.evidenceFamilyId;
    groups.set(key, [...(groups.get(key) ?? []), entry]);
  }
  const result: UpcomingResetNotice[] = [];
  for (const [key, group] of groups) {
    const timed = group.filter((entry) => {
      const time = exactPublicTime(entry.signal.source.publishedAt);
      return time != null && time <= now && time >= now - 72 * 3_600_000;
    });
    const quoted = timed.map((entry) => {
      const delivery = entry.signal.kind === "global_banked_reset_grant" ? resetDeliveryExcerpt(entry.signal.summary) : null;
      return { entry, excerpt: delivery ?? resetCommitmentExcerpt(entry.signal.summary), delivery: delivery != null };
    })
      .find((item) => item.excerpt != null);
    if (!quoted?.excerpt) continue;
    const { entry, excerpt } = quoted;
    const kinds = new Set(group.map((item) => item.signal.kind).filter((kind) => kind === "global_full_reset" || kind === "global_banked_reset_grant"));
    if (kinds.size > 1) continue; // Disagreeing reset/grant classification needs review.
    const withdrawn = latest.some((other) => isPublicWithdrawal(other)
      && group.some((item) => sameResetSubject(item.signal, other.signal)));
    const reports = latest.filter((item) => group.some((subject) => sameResetSubject(item.signal, subject.signal))
      && item.signal.semantics === "confirmed_reset")
      .flatMap((item) => {
        const at = exactPublicTime(item.signal.occurredAt);
        return at != null && at <= now ? [at] : [];
      });
    const reportedAt = reports.length ? Math.max(...reports) : null;
    const reported = reportedAt != null;
    const hints = group.flatMap((item) => {
      const hint = item.signal.announcementTiming;
      if (!hint || hint.sourceUrl !== "https://quotaresets.com/api/v1/events.json"
        || !item.signal.source.discoveredVia.includes(hint.sourceUrl)) return [];
      const exact = exactPublicTime(hint.expectedAt);
      if (exact != null) return [{ start: exact, end: exact, timeZone: hint.timeZone, sourceUrl: hint.sourceUrl,
        deadline: true, explicit: item.signal.semantics === "explicit_timed_reset" }];
      const bounds = hint.expectedOn ? sourceDateBounds(hint.expectedOn, hint.timeZone) : null;
      if (!bounds) return [];
      return [{ ...bounds, timeZone: hint.timeZone, sourceUrl: hint.sourceUrl,
        deadline: /\bby (?:midnight|(?:the )?end of (?:the )?day)\b/iu.test(excerpt), explicit: false }];
    });
    const conflictingTiming = new Set(hints.map((hint) => `${hint.start}:${hint.end}`)).size > 1;
    const timing = conflictingTiming ? null : hints[0] ?? null;
    const collectedVia = [...new Set(group.flatMap((item) => item.signal.source.discoveredVia))];
    const trackers = new Set(collectedVia.flatMap((url) => {
      try { const host = new URL(url).hostname; return ["quotaresets.com", "codex-reset.com"].includes(host) ? [host] : []; } catch { return []; }
    }));
    const relevantSources = timeline.sources.filter((source) => timing?.explicit
      ? source.sourceUrl === timing.sourceUrl : collectedVia.includes(source.sourceUrl));
    const stale = relevantSources.length === 0 || relevantSources.some((source) => {
      const time = exactPublicTime(source.lastSuccessAt);
      return source.issue != null || time == null || time > now || now - time >= 15 * 60_000;
    });
    const explicit = group.some((item) => item.signal.semantics === "explicit_timed_reset")
      && kinds.size === 1 && kinds.has("global_full_reset") && !withdrawn && !conflictingTiming;
    const cohort = group.map((item) => item.signal.announcementTiming?.cohort).find((value): value is string => !!value) ?? null;
    result.push({ key, kind: kinds.has("global_banked_reset_grant") ? "grant" : reported || explicit ? "reset" : "unknown", excerpt,
      url: entry.signal.source.canonicalUrl, publishedAt: exactPublicTime(entry.signal.source.publishedAt)!, collectedVia,
      trackerCount: trackers.size, timing, explicit, cohort, reportedAt, stale, conflictingTiming,
      state: withdrawn ? "withdrawn" : reported ? "reported" : quoted.delivery ? "announced_delivery" : timing && timing.end <= now ? "elapsed" : "upcoming" });
  }
  return result.sort((a, b) => Number(activeResetCommitment(b, now)) - Number(activeResetCommitment(a, now))
    || b.publishedAt - a.publishedAt).slice(0, 3);
}
