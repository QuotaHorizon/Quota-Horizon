import type { CSSProperties } from "react";
import type { DesktopQuotaWindow } from "../../../../capacity-preview/src/status";
import { durationLabel, localResetTime, percentLabel, resetCountdown } from "./presentation";
import styles from "./index.module.less";

export function ShortQuotaCard({ window, notApplicable, language, now }: {
  window: DesktopQuotaWindow | null;
  notApplicable: boolean;
  language: "en" | "zh";
  now: number;
}) {
  const label = window ? durationLabel(window, language) : "5h";
  const value = percentLabel(window);
  const explanation = notApplicable
    ? (language === "zh" ? "此套餐无独立 5h 限额" : "No separate 5h limit on this plan")
    : (language === "zh" ? "尚未取得此窗口额度" : "This window is not available yet");
  return (
    <div className={styles.secondary} data-unavailable={!window}>
      <div className={`${styles.ring} ${styles.compactRing}`} role="img" aria-label={`${label} ${value}`}
        style={{ "--capacity-progress": `${window?.remainingPercent ?? 0}%` } as CSSProperties}>
        <div><strong>{value}</strong><span>{label}</span></div>
      </div>
      <div className={styles.secondaryCopy}>
        <div className={styles.secondaryHeading}><span>{label}</span><strong>{value}</strong></div>
        <div className={styles.secondaryMeta}>
          <span>{window ? resetCountdown(window.resetsAt, now, language) : explanation}</span>
        </div>
        {window && <small>{localResetTime(window.resetsAt, language)}</small>}
      </div>
    </div>
  );
}
