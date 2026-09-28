import type { PublicEvidenceDisposition, PublicResetArchiveEntry, PublicTimelineEntry } from "./types";

type Signal = PublicResetArchiveEntry["signal"];
type Evidence = { signal: Signal; disposition: PublicEvidenceDisposition };

/** The domain intentionally leaves indirect negative signals as needs_review.
 * Presentation must inspect semantics too, without promoting positive claims. */
export function isPublicWithdrawal(entry: Evidence) {
  return entry.signal.scope !== "personal" && entry.signal.source.sourceClass !== "provider_ui_observation"
    && ["global_full_reset", "global_banked_reset_grant", "unclassified"].includes(entry.signal.kind)
    && (["corrected", "retracted", "negative"].includes(entry.disposition)
      || ["corrected", "retracted", "negative_signal"].includes(entry.signal.semantics ?? ""));
}

export function latestPublicEntries<T extends Evidence>(entries: T[]): T[] {
  const latest = new Map<string, T>();
  for (const entry of entries) {
    const prior = latest.get(entry.signal.signalId);
    if (!prior || entry.signal.revision > prior.signal.revision) latest.set(entry.signal.signalId, entry);
  }
  return [...latest.values()];
}

/** Explicit identity only: not similar wording or nearby dates. Known card and
 * full-reset topics stay separate even when discussed in the same post. */
export function sameResetSubject(left: Signal, right: Signal) {
  if (left.kind !== right.kind && left.kind !== "unclassified" && right.kind !== "unclassified") return false;
  return (!!left.evidenceFamilyId && left.evidenceFamilyId === right.evidenceFamilyId)
    || left.source.canonicalUrl === right.source.canonicalUrl
    || (left.kind === right.kind && left.kind !== "unclassified" && left.eventId != null && left.eventId === right.eventId);
}

export function publicPostWithdrawn(entries: PublicTimelineEntry[] | undefined, url: string | null | undefined) {
  if (!url || !entries) return false;
  const latest = latestPublicEntries(entries);
  const matching = latest.filter((entry) => entry.signal.source.canonicalUrl === url
    && entry.signal.scope !== "personal" && entry.signal.source.sourceClass !== "provider_ui_observation");
  return latest.some((entry) => isPublicWithdrawal(entry) && matching.some((subject) => sameResetSubject(subject.signal, entry.signal)));
}
