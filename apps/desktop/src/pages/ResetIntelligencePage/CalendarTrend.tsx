import type { Language } from "../../i18n";
import type { CalendarTrend as Trend } from "./quantitativeTypes";
import { percent } from "./quantitativeModel";
import styles from "./quantitative.module.less";

export function CalendarTrend({ trend, language }: { trend?: Trend | null; language: Language }) {
  if (!trend?.bins.length) return null;
  const zh = language === "zh", bins = trend.bins;
  const rolling = trend.estimate_window_hours === 6;
  const eligible = (b: Trend["bins"][number]) => b.state === "available" && b.reset_share != null
    && (rolling ? (b.sample_votes ?? 0) >= 50 : b.new_clicks >= 50 && b.coverage_fraction >= .9);
  const x = (i: number) => 56 + i * 73, y = (p: number) => 114 - p * 94;
  const maxClicks = Math.max(1, ...bins.map(b => b.new_clicks));
  const time = (at: number) => new Date(at * 1000).toLocaleTimeString(zh ? "zh-CN" : "en-GB", { hour: "2-digit", minute: "2-digit", hour12: false });
  const tail = bins.at(-1)!, previous = bins.at(-2);
  const change = trend.change, comparable = change.state === "available" && eligible(tail) && previous && eligible(previous);
  return <div className={styles.communityTrend}>
    <div className={styles.heading}><b>{zh ? "逐小时新增点击" : "New clicks by hour"}</b><span>{zh ? "过去 6h · 本地时间" : "Past 6h · local time"}</span></div>
    <svg viewBox="0 0 465 205" role="img" aria-label={zh ? "每小时重置倾向和新增点击量" : "Hourly reset preference and new click volume"}>
      {[0, .5, 1].map(p => <g key={p}><line x1="30" x2="449" y1={y(p)} y2={y(p)} /><text x="26" y={y(p) + 4} textAnchor="end">{p * 100}%</text></g>)}
      {bins.map((b, i) => {
        const valid = eligible(b), prior = bins[i - 1], height = 30 * b.new_clicks / maxClicks;
        const title = `${time(b.start)}–${time(b.end)} · ${b.new_clicks} ${zh ? "次新增点击" : "new clicks"} · ${zh ? "覆盖" : "coverage"} ${percent(b.coverage_fraction)}${rolling ? ` · 6h ${b.sample_votes ?? 0} ${zh ? "次新增点击" : "new clicks"}` : ""}${valid ? ` · ${percent(b.reset_share)}` : ` · ${zh ? "样本或覆盖待达标" : "awaiting sample or coverage"}`}`;
        return <g key={b.start}>
          {valid && prior && eligible(prior) && <path d={`M${x(i - 1)},${y(prior.reset_share!)} L${x(i)},${y(b.reset_share!)}`} className={styles.sentimentLine} />}
          {valid && <circle cx={x(i)} cy={y(b.reset_share!)} r="4" className={styles.sentimentDot}><title>{title}</title></circle>}
          <rect x={x(i) - 18} y={174 - height} width="36" height={Math.max(1, height)} rx="3" className={styles.sentimentVolume}><title>{title}</title></rect>
          <text x={x(i)} y={139} textAnchor="middle">{b.new_clicks.toLocaleString()}</text>
          <text x={x(i)} y="194" textAnchor="middle">{time(b.end)}</text>
        </g>;
      })}
    </svg>
    <p className={styles.caption}>{comparable && change.reset_share_delta != null ? <>
      <b>{rolling ? (zh ? "近期倾向 " : "Recent preference ") : (zh ? "最近 1h " : "Latest hour ")}{percent(tail.reset_share)}</b>
      {zh ? " · 较前 1h " : " · vs previous hour "}{change.reset_share_delta > 0 ? "+" : ""}{(change.reset_share_delta * 100).toFixed(1)} {zh ? "点" : "pts"}
      {change.click_rate_ratio != null && <span>{zh ? ` · 点击速度 ${change.click_rate_ratio.toFixed(2)} 倍` : ` · Click rate ${change.click_rate_ratio.toFixed(2)}×`}</span>}
    </> : rolling ? (zh ? "曲线按各时点近 6h 的新增点击计算，至少 50 次后显示。" : "Each curve point uses at least 50 new clicks in its trailing 6h.")
      : (zh ? "逐小时样本需至少 50 次新增点击和连续采集；缺口保留。" : "Each hour needs at least 50 new clicks and continuous collection; gaps remain visible.")}</p>
    <span className={styles.caption}>{rolling ? (zh ? "曲线：滚动 6h 加权倾向 · 柱：每小时新增点击" : "Line: rolling 6h weighted preference · Bars: hourly new clicks")
      : (zh ? "曲线：重置倾向 · 柱：新增点击数" : "Line: reset preference · Bars: new clicks")}</span>
  </div>;
}
