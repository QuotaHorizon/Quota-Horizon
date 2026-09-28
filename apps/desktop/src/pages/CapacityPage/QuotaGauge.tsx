import { Progress } from "antd";
import { clampPercent } from "./presentation";
import { quotaPercentLabel } from "../../utils/quotaPercent";
import styles from "./index.module.less";

export function QuotaGauge({ remaining, label }: { remaining: number | null; label: string }) {
  if (remaining == null || !Number.isFinite(remaining)) {
    return <div className={styles.unknownGauge} role="img" aria-label={`${label} —`}><strong>—</strong></div>;
  }
  const percent = clampPercent(remaining);
  return <Progress type="circle" percent={percent} size={118}
    strokeColor={percent <= 10 ? "#d14343" : percent <= 30 ? "#d89614" : "#35ada7"}
    trailColor="rgba(113, 128, 120, .16)" format={() => quotaPercentLabel(percent)} />;
}
