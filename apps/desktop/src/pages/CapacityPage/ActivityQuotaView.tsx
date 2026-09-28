import type { ActivityQuotaWindow, DesktopActiveTimeQuotaEnvelope } from "../../../../capacity-preview/src/status";
import type { Language } from "../../i18n";
import { quotaPercentLabel } from "../../utils/quotaPercent";
import { activityMessage } from "./activeTimeModel";
import { planTime } from "./demandPlanModel";
import styles from "./ActivityQuota.module.less";

export function activityQuotaReason(code:string,language:Language):string {
  const zh=language==="zh";
  switch(code) {
    case "balance_increased":return zh?"期间额度回升，暂不汇总消耗。":"Quota rose during this interval. Consumption is not totaled.";
    case "window_changed":return zh?"额度窗口时长或重置边界发生变化，不跨窗口比较。":"The window duration or reset boundary changed; separate windows are not compared.";
    case "missing_reset_time":return zh?"快照缺少重置时间，无法确认是否处于同一窗口。":"Reset times are missing, so a shared quota window cannot be verified.";
    case "expired_window":return zh?"快照中的额度窗口已到期，暂不计算下降。":"A captured quota window was already expired; no decrease is calculated.";
    case "observation_gap":return zh?"采样间隔超过 30 分钟，不跨越缺口汇总下降。":"A gap exceeds 30 minutes; decreases are not totaled across it.";
    case "query_truncated":return zh?"这段历史超过本次读取上限，未用截断数据计算下降。":"This history exceeds the read limit; truncated data is not used to calculate a decrease.";
    case "conflicting_samples":return zh?"同一采样时刻存在冲突数值，需要先检查数据。":"Conflicting values share a capture time; the data needs review.";
    case "incompatible_source":return zh?"部分来源状态或数值不可用于比较，暂不计算下降。":"Some source states or values cannot be compared; no decrease is calculated.";
    default:return zh?"至少需要两个采样时刻才能比较。":"At least two capture times are needed for comparison.";
  }
}
function duration(seconds:number,zh:boolean):string {
  if (seconds<60) return `${seconds} ${zh?"秒":"sec"}`;
  const minutes=Math.floor(seconds/60),rest=seconds%60;
  return `${minutes} ${zh?"分钟":"min"}${rest?` ${rest} ${zh?"秒":"sec"}`:""}`;
}
function windowTitle(minutes:number|null,zh:boolean):string {
  if (minutes===10080) return zh?"每周额度":"Weekly quota";
  if (minutes===300) return zh?"5 小时额度":"5-hour quota";
  if (!minutes) return zh?"窗口信息变化或不完整":"Window changed or incomplete";
  return minutes%1440===0?`${minutes/1440} ${zh?"天窗口":"day window"}`:minutes%60===0?`${minutes/60} ${zh?"小时窗口":"hour window"}`:`${minutes} ${zh?"分钟窗口":"minute window"}`;
}
function Window({value,language}:{value:ActivityQuotaWindow;language:Language}) {
  const zh=language==="zh";
  const comparable=value.reasonCode==="comparable" && value.observedDecreasePercent!==null;
  const endpoints=["comparable","observation_gap","balance_increased","insufficient_samples"].includes(value.reasonCode);
  const coverage=value.coveragePercent.toLocaleString(zh?"zh-CN":"en-US",{maximumFractionDigits:1});
  return <li className={styles.window}>
    <div className={styles.top}><strong>{windowTitle(value.windowMinutes,zh)}</strong>{endpoints && <span className={styles.balance}>{quotaPercentLabel(value.firstRemainingPercent)} → {quotaPercentLabel(value.lastRemainingPercent)}</span>}</div>
    {comparable ? <p className={styles.result}>{value.observedDecreasePercent===0 ? (zh?"采样期间余额未见变化。":"No balance change was observed between these captures.") : (zh?`采样期间观测下降 ${quotaPercentLabel(value.observedDecreasePercent).replace("%","")} 个百分点`:`Observed drop between captures: ${quotaPercentLabel(value.observedDecreasePercent).replace("%","")} percentage points`)}</p> : <p className={styles.notice}>{activityQuotaReason(value.reasonCode,language)}</p>}
    <p className={styles.muted}>{planTime(value.observedFrom,language)} → {planTime(value.observedUntil,language)}</p>
    <div className={styles.metrics}><span>{zh?`${value.sampleCount} 个采样时刻`:`${value.sampleCount} capture times`}</span><span>{zh?`覆盖记录时段 ${coverage}%`:`Covers ${coverage}% of the recorded interval`}</span><span>{zh?`最大间隔 ${duration(value.maximumGapSeconds,zh)}`:`Largest gap ${duration(value.maximumGapSeconds,zh)}`}</span></div>
    {(value.leadingGapSeconds>0 || value.trailingGapSeconds>0) && <p className={styles.muted}>{zh?`起始未覆盖 ${duration(value.leadingGapSeconds,zh)}，末尾未覆盖 ${duration(value.trailingGapSeconds,zh)}；没有补算。`:`Uncovered start: ${duration(value.leadingGapSeconds,zh)}; end: ${duration(value.trailingGapSeconds,zh)}. Neither is estimated.`}</p>}
    {value.compatibilityUnverified && <p className={styles.muted}>{zh?"部分采样的读取器兼容性待验证。":"Reader compatibility is unverified for some captures."}</p>}
  </li>;
}
export function ActivityQuotaView({value,language,failed=false,loading=false,archived=false,onRetry}:{value:DesktopActiveTimeQuotaEnvelope|null;language:Language;failed?:boolean;loading?:boolean;archived?:boolean;onRetry:()=>void}) {
  const zh=language==="zh";const available=value?.status==="available" && value.comparison;
  return <section className={styles.panel} aria-label={zh?"同期额度":"Quota during this interval"}>
    <div className={styles.top}><strong>{zh?"同期额度":"Quota during this interval"}</strong><button disabled={loading} onClick={onRetry}>{loading?(zh?"读取中…":"Reading…"):(zh?"重新读取":"Read again")}</button></div>
    {!value && !failed && <p className={styles.muted}>{zh?"读取本机原始快照…":"Reading original local captures…"}</p>}
    {failed && <p className={styles.notice} role="status">{zh?"暂时无法读取同期历史。已有内容是上次结果，请重试。":"Could not read this interval’s history. Any retained content is the previous result; please retry."}</p>}
    {value && !available && <p className={styles.muted}>{value.status==="missing"?(zh?"当前账号和读取环境中找不到这条记录，请返回刷新记录列表。":"This record was not found in the current account and reader environment. Refresh the records list."):activityMessage(value.reasonCode,language)}</p>}
    {available && <>
      {archived && <p className={styles.notice}>{zh?"下方只比较该记录原读取环境的快照，不混入当前读取器的额度。":"Only captures from this record’s original reader are compared; current-reader quota is not mixed in."}</p>}
      {!value.record?.included && <p className={styles.notice}>{zh?"此记录已排除，以下为同期额度。":"This record is excluded. Quota over the same period is shown below."}</p>}
      {value.comparison!.queryTruncated && <p className={styles.notice}>{activityQuotaReason("query_truncated",language)}</p>}
      {value.comparison!.windows.length===0 ? <p className={styles.muted}>{zh?"该时段没有额度记录。":"No quota records are available for this period."}</p> : <ul>{value.comparison!.windows.map(window=><Window key={window.limitId} value={window} language={language}/>)}</ul>}
      <p className={styles.muted}>{zh?"显示记录起止时段内的账号额度变化，包含暂停期间及其他设备的使用。":"Shows account-wide quota changes over the recorded period, including pauses and use on other devices."}</p>
      <small className={styles.muted}>{zh?"读取于 ":"Read at "}{planTime(value.generatedAt,language)}</small>
    </>}
  </section>;
}
