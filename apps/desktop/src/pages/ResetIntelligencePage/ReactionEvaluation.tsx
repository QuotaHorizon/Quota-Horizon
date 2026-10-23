import type { Language } from "../../i18n";
import type { QuantitativeView } from "./quantitativeTypes";
import { percent, sourceNames } from "./quantitativeModel";
import { publicEvidenceTime } from "./presentation";
import styles from "./quantitative.module.less";

const duration = (seconds: number | null, zh: boolean) => seconds == null || !Number.isFinite(seconds) ? "—"
  : seconds < 60 ? `${seconds.toFixed(1)} ${zh ? "秒" : "s"}`
  : seconds < 3600 ? `${(seconds / 60).toFixed(1)} ${zh ? "分钟" : "min"}`
  : `${(seconds / 3600).toFixed(1)} h`;

export function ReactionEvaluation({ view, language }: { view: QuantitativeView; language: Language }) {
  const zh = language === "zh", events = view.evaluation?.reaction?.events ?? [], p = view.presentation;
  if (!events.length && !p) return null;
  return <details className={styles.disclosure}>
    <summary>{zh ? "重置后的响应速度" : "Response after a reset"}</summary>
    {p?.presentedAt != null && <p className={styles.caption}>
      {zh ? "本次页面接收 → 更新：" : "Current page receipt → update: "}{Math.max(0, p.presentedAt-p.receivedAt).toFixed(0)} ms
      {p.visible === false ? (zh ? " · 后台页面" : " · background page") : ""}
      {zh ? " · 对应预测 " : " · Forecast "}{publicEvidenceTime(p.issuedAt, language)}
    </p>}
    {events.slice(0, 3).map(event => <section key={event.id}>
      <h3>{event.kind === "banked" ? (zh ? "重置卡" : "Reset credit") : (zh ? "自动重置" : "Automatic reset")}
        {" · "}{publicEvidenceTime(new Date(event.completed_to*1000).toISOString(), language)}
        {event.status === "pending" ? (zh ? " · 完成报告待核验" : " · reported completion under review") : ""}</h3>
      <div className={styles.tableScroll}><table><thead><tr>{(zh
        ? ["来源 / 范围", "更新状态", "来源发布 → 采集", "已知完成 → 预测更新", "旧周期持续", "24h 前 → 后"]
        : ["Source / scope", "Update state", "Publication → capture", "Known completion → update", "Old cycle duration", "24h before → after"])
        .map(label => <th key={label}>{label}</th>)}</tr></thead><tbody>{event.series.map(row => <tr key={`${row.source}:${row.target}:${row.track}`}>
          <td>{sourceNames[row.source] ?? row.source}<small>{row.target === "automatic_reset" ? (zh ? "自动重置" : "Automatic") : (zh ? "重置或发卡" : "Reset or credit")}</small></td>
          <td>{({ updated: zh ? "已更新" : "Updated", awaiting_update: zh ? "待更新" : "Awaiting update",
            anchor_unknown: zh ? "事件对应待核验" : "Event match unavailable", no_prior_observation: zh ? "缺少之前记录" : "No earlier observation" })[row.state]}</td>
          <td>{duration(row.publication_to_capture_seconds, zh)}</td>
          <td>{duration(row.known_to_update_seconds, zh)}</td>
          <td>{row.residual_censored ? "≥ " : ""}{duration(row.old_cycle_residual_seconds, zh)}</td>
          <td>{percent(row.before_probability_24h)} → {percent(row.after_probability_24h)}</td>
        </tr>)}</tbody></table></div>
    </section>)}
    <p className={styles.caption}>{zh ? "按事件对应关系核验更新，缺少时间记录的环节显示 —。" : "Updates are checked against the event identity. Stages without recorded timestamps show —."}</p>
  </details>;
}
