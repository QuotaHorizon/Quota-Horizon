import type {PaceEvidenceCounts, PaceEvidenceSummary} from "../../../../capacity-preview/src/status";
import type {Language} from "../../i18n";
import {planTime} from "./demandPlanModel";
import styles from "./CapacityPace.module.less";

function fraction(numerator: number, denominator: number, language: Language) {
  if (!denominator) return "—";
  return `${new Intl.NumberFormat(language === "zh" ? "zh-CN" : "en", {maximumFractionDigits: 1}).format(100 * numerator / denominator)}% (${numerator}/${denominator})`;
}
function Counts({counts,language}: {counts:PaceEvidenceCounts;language:Language}) {
  const zh=language==="zh";
  const items = [
    {key:"within",count:counts.within,label:zh?"范围内":"Within band"},
    {key:"below",count:counts.below,label:zh?"余额更低 · 偏乐观":"Lower balance · optimistic"},
    {key:"above",count:counts.above,label:zh?"余额更高 · 偏保守":"Higher balance · conservative"},
    {key:"unscorable",count:counts.unscorable,label:zh?"无法核验":"Unscorable"},
    {key:"pending",count:counts.pending,label:zh?"仍待核验":"Pending"},
  ];
  return <>
    {!!counts.total && <div className={styles.evidenceBar} aria-hidden="true">{items.filter(v=>v.count>0).map(v=><span key={v.key} data-kind={v.key} style={{flex:v.count}}/>)}</div>}
    <div className={styles.evidenceCounts}>{items.map(v=><div key={v.key}><strong>{v.count}</strong><small>{v.label}</small></div>)}</div>
  </>;
}
export function PaceEvidenceView({value,language,reasonText}: {value:PaceEvidenceSummary;language:Language;reasonText:(reason:string)=>string}) {
  const zh=language==="zh", counts=value.counts;
  const comparable=counts.within+counts.below+counts.above;
  const terminal=comparable+counts.unscorable;
  const missing=value.unscorableReasons.filter(v=>v.reasonCode==="target_samples_missing"||v.reasonCode==="observation_gap").reduce((sum,v)=>sum+v.count,0);
  const pair=(v:[number,number]|null,divisor=1)=>v?Array.from(new Set(v.map(n=>new Intl.NumberFormat(zh?"zh-CN":"en",{maximumFractionDigits:1}).format(n/divisor)))).join("–"):"—";
  return <section className={styles.evidence} aria-label={zh?"前瞻记录汇总":"Prospective evidence summary"}>
    <div className={styles.heading}><strong>{zh?"这些推算后来怎样了":"How these scenarios turned out"}</strong><small>{zh?`${counts.total} 条记录`:`${counts.total} records`}</small></div>
    <p className={styles.muted}>{value.limited
      ? (zh?`汇总最近 ${counts.total} 条，较早记录可翻页查看。`:`Summarizing the latest ${counts.total} records. Earlier records are available on the following pages.`)
      : (zh?"当前账号与读取器的本机已保留记录汇总，不限于本页。":"All locally retained records for this account and reader, not just this page.")}</p>
    {value.oldestIssuedAt && value.newestIssuedAt && <small>{zh?"记录生成范围：":"Issue dates: "}{planTime(value.oldestIssuedAt,language)} → {planTime(value.newestIssuedAt,language)}</small>}
    <Counts counts={counts} language={language}/>
    <div className={styles.metrics}><span>{zh?"范围内 / 可比较：":"Within / comparable: "}{fraction(counts.within,comparable,language)}</span><span>{zh?"无法核验 / 已终结：":"Unscorable / terminal: "}{fraction(counts.unscorable,terminal,language)}</span><span>{zh?"缺读数或断档 / 已终结：":"Missing captures or gaps / terminal: "}{fraction(missing,terminal,language)}</span></div>
    <p className={styles.muted}>{zh?"统计已保存的实验推算：余额更低为偏乐观，更高为偏保守；待核验记录单列。":"Summary of saved experimental scenarios: lower balances indicate optimistic estimates, higher balances conservative estimates. Pending records are listed separately."}</p>
    {!!value.groups.length && <details className={styles.evidenceGroups}><summary>{zh?"按额度窗口与证据条件查看":"By quota window and evidence conditions"}</summary><div className={styles.trialList}>{value.groups.map((group,index)=><article className={styles.trial} key={index}>
      <strong>{group.windowMinutes===10080?(zh?"周额度":"Weekly quota"):group.windowMinutes===300?(zh?"5 小时额度":"5-hour quota"):(zh?`额度窗口（${group.windowMinutes??"—"} 分钟）`:`Quota window (${group.windowMinutes??"—"} min)`)} · {group.algorithmVersion==="recent-block-pace-experimental-v1"&&group.verifierVersion==="pace-balance-outcome-v1"?(zh?"近期节奏实验 1":"Recent pace experiment 1"):(zh?"其他实验版本":"Other experiment version")} · {group.compatibilityUnverified?(zh?"来源兼容性未验证":"Source compatibility unverified"):(zh?"来源兼容性已测":"Source compatibility tested")}</strong>
      <Counts counts={group.counts} language={language}/>
      <small>{zh?"原输入的最大采样间隔：":"Original inputs’ largest capture gaps: "}{pair(group.trainingMaximumGapSeconds,60)} {zh?"分钟":"min"} · {zh?"原速率高低比：":"Original high/low rate ratios: "}{pair(group.trainingRateRatio)}</small>
    </article>)}</div><p className={styles.muted}>{zh?"采样间隔和速率比显示原始输入的最小—最大值。":"Capture gaps and rate ratios show the minimum and maximum across original inputs."}</p></details>}
    {!!value.unscorableReasons.length && <details className={styles.evidenceGroups}><summary>{zh?"哪些记录无法核验":"Why records could not be scored"}</summary><ul>{value.unscorableReasons.map((reason,index)=><li key={index}>{reason.count} {zh?"条：":"records: "}{reasonText(reason.reasonCode)}</li>)}</ul></details>}
    <small>{zh?"汇总读取于 ":"Summary read "}{planTime(value.generatedAt,language)}</small>
  </section>;
}
