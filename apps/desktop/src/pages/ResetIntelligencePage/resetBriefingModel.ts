import type { Language } from "../../i18n";
import { resetDeliveryExcerpt } from "./upcomingResetModel";
import { publicPostWithdrawn } from "./publicCorrections";
import type { PublicInsights, PublicPost, PublicResetTimeline } from "./types";

const halfHour = 30 * 60_000;
export function forecastPresentation(insights: PublicInsights | undefined, now: number, failed = false, timeline?: PublicResetTimeline | null) {
  const state = insights?.forecast;
  const value = state?.value ?? null;
  const age = value ? now - Date.parse(value.updatedAt) : NaN;
  const checkedAge = value ? now - Date.parse(value.checkedAt) : NaN;
  const stale = !!value && (failed || !!state?.issue || !Number.isFinite(age) || age < -60_000 || age >= halfHour
    || !Number.isFinite(checkedAge) || checkedAge < -60_000 || checkedAge >= halfHour);
  const deadline = value?.signalDeadline ? Date.parse(value.signalDeadline) : NaN;
  const signalAge = value?.signalPublishedAt ? now - Date.parse(value.signalPublishedAt) : NaN;
  const signalWithdrawn = !!value && (value.signalCorrected || publicPostWithdrawn(timeline?.entries, value.signalUrl));
  const signalActive = !!value && !stale && !signalWithdrawn && Number.isFinite(signalAge) && signalAge >= -60_000
    && signalAge < 72 * 3_600_000 && Number.isFinite(deadline) && deadline > now;
  return { value, stale, signalActive, signalWithdrawn };
}

export function resetRelated(post: PublicPost) {
  return ["grant", "reset_report", "notice", "limits"].includes(post.kind)
    || /\b(?:reset|banked|quota|usage limits?)\b/iu.test(post.text)
    // Parent context alone does not make a casual reply a useful reset update.
    // Keep contextual timing/commitment replies; all other posts remain in All.
    || (/\b(?:reset|banked|quota)\b/iu.test(post.parent?.text ?? "")
      && /\b(?:coming|arriv\w*|load\w*|grant\w*|issu\w*|tomorrow|tonight|today|monday|tuesday|wednesday|thursday|friday|saturday|sunday|midnight)\b/iu.test(post.text));
}

/** A bounded editorial explanation, not a probabilistic classifier. The actual
 * reply and parent stay visible so this interpretation is inspectable. */
export function postReading(post: PublicPost, language: Language, timeline?: PublicResetTimeline | null) {
  const zh = language === "zh";
  const corrected = publicPostWithdrawn(timeline?.entries, post.url);
  if (corrected) return { tone: "notice", title: zh ? "相关追踪记录已更正" : "Related tracker record corrected",
    meaning: zh ? "此条预告已失效，可在情报变化中查看更正内容。" : "This notice is no longer active. See source updates for the correction." };
  if (post.isReply && post.parent?.text && /\bbanked reset|reset (?:card|credit)\b/iu.test(post.parent.text)) return {
    tone: "grant", title: zh ? "重置卡相关回复" : "Reset-card reply",
    meaning: zh ? "回复上文讨论的重置卡，完整对话见下方。" : "A reply about reset cards. The full conversation is below.",
  };
  if (post.kind === "grant") return { tone: "grant",
    title: resetDeliveryExcerpt(post.text) ? (zh ? "Tibo 宣布开始发放重置卡" : "Tibo announces reset-card distribution")
      : (zh ? "Tibo 的重置卡动态" : "Tibo’s reset-card update"),
    meaning: zh ? "重置卡可手动使用；可用数量见「当前账号」。" : "Reset cards are redeemed manually. See Your account for the available count.",
  };
  if (post.kind === "reset_report") return { tone: "reset", title: zh ? "Tibo 的额度重置声明" : "Tibo’s quota reset statement",
    meaning: zh ? "适用范围和进展见原文。" : "See the statement for scope and progress." };
  if (post.kind === "limits") return { tone: "notice", title: zh ? "额度规则动态" : "Quota policy update",
    meaning: zh ? "套餐或用量规则的最新消息。" : "Updates to plans or usage limits." };
  if (post.kind === "notice") return { tone: "notice", title: zh ? "Tibo 的重置动态" : "Tibo’s reset update",
    meaning: zh ? "重置类型与时间待明确，原文如下。" : "Reset type and timing are unspecified. The original is below." };
  return { tone: "notice", title: zh ? "Tibo 的公开回复与讨论" : "Tibo’s public replies and discussion",
    meaning: post.isReply && !post.parent?.text
      ? (zh ? "回复上文暂缺。" : "Reply context is unavailable.")
      : (zh ? "相关讨论，完整内容见下方。" : "Related discussion. The full conversation is below."),
  };
}
