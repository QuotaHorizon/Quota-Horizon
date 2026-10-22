import type { Language } from "../../i18n";
import { publicEvidenceTime } from "./presentation";
import { percent } from "./quantitativeModel";
import type { QuantReport } from "./quantitativeTypes";
import { CalendarTrend } from "./CalendarTrend";
import styles from "./quantitative.module.less";

/** Calendar-day preferences retain their own window and independent evaluation. */
export function CalendarSentiment({ report, language, now }: {
  report: QuantReport | null; language: Language; now: number;
}) {
  const data = report?.calendar_sentiment;
  if (!data) return null;
  const zh = language === "zh", seconds = now / 1000;
  const current = data.latest.filter(row => row.source === "didcodex_poll"
    && row.window_start <= seconds && seconds < row.window_end
    && row.state !== "closed" && row.state !== "upcoming")
    .sort((a, b) => b.observed_at - a.observed_at)[0];
  const reportTime = Date.parse(report.generated_at);
  const stale = current && (current.state === "stale" || seconds - current.observed_at > 180
    || current.observed_at > seconds + 60 || !Number.isFinite(reportTime)
    || now - reportTime > 180_000 || reportTime > now + 60_000);
  const insufficient = current && (current.recent_votes < 50 || current.state === "insufficient_recent_votes");
  const number = (value: number) => value.toLocaleString(zh ? "zh-CN" : "en-US");
  return <section className={styles.communityInputs} aria-label={zh ? "活动日重置倾向" : "Activity-day reset preference"}>
    <h3>{zh ? "活动日重置倾向" : "Activity-day reset preference"} · Did Codex Reset</h3>
    {current ? <div className={styles.communityInput}>
      <div>
        <b>{current.date} · {zh ? "太平洋时间" : "Pacific time"}</b>
        <span>{zh ? `近 6h 新增 ${number(current.recent_votes)} 次点击 · 累计 ${number(current.total_clicks)}`
          : `${number(current.recent_votes)} new clicks in 6h · ${number(current.total_clicks)} total`}</span>
        <small>{zh ? "采集于 " : "Observed "}<time dateTime={new Date(current.observed_at * 1000).toISOString()}>
          {publicEvidenceTime(new Date(current.observed_at * 1000).toISOString(), language)}</time></small>
      </div>
      <div>
        <strong>{percent(current.weighted_reset_share)}</strong>
        <small>{zh ? "近期点击按时间加权" : "Recent clicks, weighted by age"}</small>
        <span>{zh ? "原始倾向 " : "Raw preference "}{percent(current.reset_share)}</span>
      </div>
    </div> : null}
    {current && <CalendarTrend trend={current.trend} language={language} />}
    <p className={styles.coverage}>
      {!current ? (zh ? "当前活动日等待新数据" : "Awaiting data for the current activity day")
        : stale ? (zh ? "采集待更新 · 上次记录" : "Collection update due · saved observation")
        : insufficient ? (zh ? "样本待达标 · 近 6h 需新增至少 50 次点击" : "Awaiting at least 50 new clicks in 6h")
        : (zh ? "活动日候选 · 独立评估中" : "Calendar-day candidate · independent evaluation")}
      <span>{zh ? "综合判断权重 " : "Weight in the combined forecast "}{percent(data.production_weight)}</span>
      {(data.frozen_days > 0 || data.scored_days > 0) && <span>
        {zh ? `已冻结 ${number(data.frozen_days)} 日 / ${number(data.frozen_trials)} 个时点 · 已评估 ${number(data.scored_days)} 日 / ${number(data.scored_trials)} 个时点`
          : `Frozen: ${number(data.frozen_days)} days / ${number(data.frozen_trials)} checkpoints · Scored: ${number(data.scored_days)} days / ${number(data.scored_trials)} checkpoints`}
      </span>}
    </p>
  </section>;
}
