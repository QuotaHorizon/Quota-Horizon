import type { DesktopStatusEnvelope } from "../../../../capacity-preview/src/status";
import type { Language } from "../../i18n";

/** Use the credit capture, not a newer quota-only cache, to describe freshness. */
export function resetCreditPresentation(envelope: DesktopStatusEnvelope | null, language: Language) {
  const zh = language === "zh";
  const status = envelope?.status;
  const credits = status?.resetCredits;
  const count = credits?.summaryStatus !== "unavailable" && credits?.availableCount != null
    && Number.isSafeInteger(credits.availableCount) && credits.availableCount >= 0 ? credits.availableCount : null;
  const live = !!status && status.dataStatus.freshness === "live"
    && envelope?.lifecycle === "ready" && !envelope.issue;
  if (count !== null) return { count, label: live ? (zh ? "可用" : "Available") : (zh ? "上次读数" : "Saved count"),
    message: live ? (zh ? "可在 Codex 中手动使用。" : "Redeem manually in Codex.")
      : (zh ? "显示上次读取的卡数。" : "Showing the last saved card count.") };
  if (!live) return { count, label: zh ? "尚未取得" : "Not read yet",
    message: zh ? "尚未取得重置卡数量。" : "The reset-card count is not available yet." };
  return { count, label: zh ? "来源未提供" : "Not supplied",
    message: envelope?.selectedExecutableSource === "cli"
      ? (zh ? "当前 CLI 未提供重置卡数据，请使用 Codex App 内置读取组件。"
        : "This CLI did not supply reset-card data. Use the Codex App’s bundled reader.")
      : (zh ? "当前读取组件未提供重置卡数据。"
        : "The current reader did not supply reset-card data.") };
}
