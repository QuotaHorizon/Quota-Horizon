import type { PublicResetArchiveEntry } from "./types";

/** Shared by the notice and calendar: an announced rollout is neither a
 * promised future drop nor proof of delivery to any particular account. */
export function resetDeliveryExcerpt(text: string): string | null {
  const parts = text.split(/([.!?](?:\s+|$)|\n+)/u);
  for (let index = 0; index < parts.length; index += 2) {
    const sentence = `${parts[index]}${parts[index + 1] ?? ""}`.trim();
    if (sentence.length > 360 || /\b(?:not|never|no|might|may|could|would|hope|wish|if|tomorrow|soon|next|tonight)\b/iu.test(sentence)
      || sentence.includes("?")) continue;
    if (/^we are (?:also )?(?:loading|granting|issuing|rolling out)\b[^.!?]{0,120}\b(?:banked reset|reset credits?)\b/iu.test(sentence)) return sentence;
  }
  return null;
}

export function isTiboStatement(signal: PublicResetArchiveEntry["signal"]) {
  try {
    const url = new URL(signal.source.canonicalUrl);
    return url.protocol === "https:" && url.hostname === "x.com" && !url.username && !url.password && !url.port
      && /^\/thsottiaux\/status\/\d+$/u.test(url.pathname)
      && signal.scope !== "personal" && signal.source.sourceClass === "official_social";
  } catch { return false; }
}

export function broadGrantDeliveryExcerpt(signal: PublicResetArchiveEntry["signal"], text: string) {
  if (signal.kind !== "global_banked_reset_grant" || !isTiboStatement(signal)
    || ["explicit_future_reset", "explicit_timed_reset", "negative_signal", "corrected", "retracted"].includes(signal.semantics ?? "")) return null;
  const excerpt = resetDeliveryExcerpt(text);
  // A grant to one person must not become a broad public grant record.
  return excerpt && /\b(?:to|into|for) (?:all|every|each)\b[^.!?]{0,160}\b(?:accounts?|users?|subscribers?|subscriptions?|plans?)\b/iu.test(excerpt)
    ? excerpt : null;
}
