import type { Language } from "../i18n";
import type { ResetCredit } from "../types";
import { formatSystemTime } from "./format";

export type ResetCreditExpiry = {
  nextExpiresAt?: string | null;
  knownExpiryCount?: number;
  expiryDetailsComplete?: boolean;
};

/** Unknown availability and missing dates never become an upcoming card expiry. */
export function summarizeResetCreditExpiry(credits: ResetCredit[], now = Date.now()): ResetCreditExpiry {
  const available = credits.filter((credit) => credit.available !== false);
  const dates = available.filter((credit) => credit.available === true)
    .map((credit) => Date.parse(credit.expiresAt ?? ""))
    .filter((date) => Number.isFinite(date) && date > now).sort((left, right) => left - right);
  return {
    nextExpiresAt: dates.length ? new Date(dates[0]).toISOString() : null,
    knownExpiryCount: dates.length,
    expiryDetailsComplete: available.length === dates.length,
  };
}

export function resetCreditExpiryHint(summary: ResetCreditExpiry, count: number, language: Language,
  { saved = false, now = Date.now(), capturedAt }: { saved?: boolean; now?: number; capturedAt?: string } = {}) {
  if (count <= 0) return null;
  const zh = language === "zh";
  const expiry = Date.parse(summary.nextExpiresAt ?? "");
  const known = summary.knownExpiryCount ?? 0;
  const complete = summary.expiryDetailsComplete === true && known === count;
  const prefix = saved ? (zh ? "记录 · " : "Saved · ") : "";
  const capture = capturedAt ? (zh ? `读取于 ${formatSystemTime(capturedAt, language)}`
    : `Read ${formatSystemTime(capturedAt, language)}`) : null;
  if (!Number.isFinite(expiry) || expiry <= now) {
    const elapsed = Number.isFinite(expiry) && expiry <= now;
    const text = elapsed ? (zh ? "到期信息待更新" : "Expiry needs refresh")
      : (zh ? "到期时间未知" : "Expiry unknown");
    return { text: prefix + text, title: [text, capture].filter(Boolean).join(" · "), urgent: false };
  }
  const date = new Date(expiry);
  const today = new Date(now);
  const sameDay = date.getFullYear() === today.getFullYear() && date.getMonth() === today.getMonth()
    && date.getDate() === today.getDate();
  const time = date.toLocaleTimeString(zh ? "zh-CN" : "en-US", { hour: "2-digit", minute: "2-digit", hour12: false });
  const shortDate = sameDay ? (zh ? `今天 ${time}` : `Today ${time}`)
    : date.toLocaleDateString(zh ? "zh-CN" : "en-US", { month: "numeric", day: "numeric",
      ...(date.getFullYear() !== today.getFullYear() ? { year: "numeric" as const } : {}) });
  const text = zh ? `${complete ? "" : "已知 "}${shortDate} 到期`
    : `${complete ? "Expires" : "Known expiry"} ${shortDate}`;
  const detail = zh ? `${complete ? "最近到期" : "最近已知到期"} ${formatSystemTime(summary.nextExpiresAt, language)}`
    : `${complete ? "Next expiry" : "Next known expiry"} ${formatSystemTime(summary.nextExpiresAt, language)}`;
  const coverage = complete ? null : (zh ? `${count} 张中 ${known} 张有有效期限` : `${known} of ${count} cards have a known future expiry`);
  return { text: prefix + text, title: [detail, coverage, capture].filter(Boolean).join(" · "),
    urgent: !saved && expiry - now <= 24 * 60 * 60 * 1_000 };
}
