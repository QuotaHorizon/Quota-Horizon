import {useEffect, useMemo, useSyncExternalStore} from "react";
import type {DesktopPaceTrialsEnvelope, PaceTrialView} from "../../../../capacity-preview/src/status";
import type {Language} from "../../i18n";
import {createPaceTrialStore} from "./paceTrialStore";
import {paceMessage, paceRangeLabel} from "./CapacityPaceView";
import {planTime} from "./demandPlanModel";
import {PaceEvidenceView} from "./PaceEvidenceView";
import styles from "./CapacityPace.module.less";

function range(value: [number, number] | null, language: Language) {
  return value ? `${paceRangeLabel({status:"available",lower:value[0],upper:value[1],reasonCode:""},language)}%` : "—";
}
export function trialReason(reason: string, language: Language) {
  const zh = language === "zh";
  const text: Record<string, [string,string]> = {
    target_samples_missing: ["目标时点附近缺少前后读数，无法核验，不计为命中。", "No bracketing captures near the target time; unscorable, not a hit."],
    boundary_uncertain: ["前后采样只能界定一个实际区间，与原范围部分交叠，无法确定是否落在范围内。", "Bracketing captures only bound the actual balance. Partial overlap makes the result inconclusive."],
    unsupported_algorithm: ["该实验版本暂无对应核验器，未用新算法改写旧结果。", "No verifier for this experiment version. New algorithms do not rewrite earlier results."],
    frozen_inputs_invalid: ["保存的输入与原推算不一致，已拒绝评分。", "Saved inputs do not reproduce the original scenario; no score is assigned."],
  };
  return text[reason]?.[zh ? 0 : 1] ?? paceMessage(reason, language);
}
function Trial({value,language,now,archived}: {value:PaceTrialView;language:Language;now:number;archived:boolean}) {
  const zh=language==="zh", outcome=value.outcome;
  const label: Record<string,[string,string]> = {within_band:["落在原范围内","Within the original band"],below_band:["实际余额更低","Actual balance lower"],above_band:["实际余额更高","Actual balance higher"]};
  const badge=outcome ? label[outcome.classification]?.[zh?0:1] ?? (zh?"无法核验":"Unscorable") : (zh?"等待核验":"Awaiting check");
  const window=value.windowMinutes===10080?(zh?"周额度":"Weekly quota"):value.windowMinutes===300?(zh?"5 小时额度":"5-hour quota"):(zh?"额度窗口":"Quota window");
  return <article className={styles.trial}>
    <div className={styles.heading}><strong>{window}</strong><span className={styles.badge} data-state={outcome?.classification ?? "pending"}>{badge}</span></div>
    <small>{zh?"生成于 ":"Issued "}{planTime(value.issuedAt,language)} · {value.algorithmVersion==="recent-block-pace-experimental-v1"?(zh?"近期节奏实验 1":"Recent pace experiment 1"):(zh?"其他实验版本":"Other experiment version")}</small>
    <p>{zh?"核验目标：":"Target: "}{planTime(value.forecastHorizon,language)}</p>
    <div className={styles.comparison}><div><small>{zh?"当时的余额情景":"Original balance scenario"}</small><strong>{range(value.balanceRange,language)}</strong></div><div><small>{zh?"后来观测到的余额":"Subsequently observed balance"}</small><strong>{range(outcome?.observedRange ?? null,language)}</strong></div></div>
    {outcome ? <>
      {outcome.observedFrom && <p className={styles.muted}>{zh?"目标附近采样：":"Captures bracketing the target: "}{planTime(outcome.observedFrom,language)}{outcome.observedUntil!==outcome.observedFrom && <> → {planTime(outcome.observedUntil,language)}</>}</p>}
      {outcome.classification==="unscorable" && <p>{trialReason(outcome.reasonCode,language)}</p>}
      <small>{zh?"核验于 ":"Checked "}{planTime(outcome.assessedAt,language)}</small>
    </> : <p className={styles.muted}>{archived ? (zh?"该旧环境未完成核验。":"The original reader did not complete this check.") : Date.parse(value.forecastHorizon)<=now ? (zh?"目标已到，但本版本的后台核验已暂停。这条旧记录尚无核验结果。":"The target has passed, but background evaluation is suspended in this version. This saved trial has no outcome.") : (zh?"后台核验已暂停，显示已保存的推算。":"Background evaluation is paused. Showing saved scenarios.")}</p>}
    {(value.compatibilityUnverified || outcome?.compatibilityUnverified) && <small>{zh?"来源兼容性待验证。":"Source compatibility is unverified."}</small>}
  </article>;
}
export function PaceTrialHistoryView({value,context,offset,failed,loading,language,archived=false,now=Date.now(),onReload,onPage}: {value:DesktopPaceTrialsEnvelope|null;context:string;offset:number;failed:boolean;loading:boolean;language:Language;archived?:boolean;now?:number;onReload:()=>void;onPage:(offset:number)=>void}) {
  const zh=language==="zh", valid=value?.historyContextId===context && value.offset===offset;
  const trials=valid && value.status==="available" ? value.trials : [];
  const checked=trials.filter(v=>v.outcome && ["within_band","below_band","above_band"].includes(v.outcome.classification)).length;
  const skipped=trials.filter(v=>v.outcome && !["within_band","below_band","above_band"].includes(v.outcome.classification)).length;
  return <div className={styles.trialBody}>
    <p className={styles.muted}>{archived ? (zh?"所选旧环境的推算与核验归档。":"Archived scenarios and outcomes from the selected earlier reader.") : (zh?"当前账号的实验推算记录。后台试算与核验已暂停。":"Experimental scenarios for this account. Background trials and evaluation are paused.")}</p>
    {failed && <p role="status">{zh?"读取失败，暂保留本页上次记录；请重试。":"Read failed. This page’s previous records are retained; please retry."}</p>}
    {valid && value.recordingFailed && <p role="status">{zh?"最近一次自动记录或核验失败，额度监控不受影响；下次采集会重试。":"The last recording or check failed. Quota monitoring is unaffected; the next capture retries."}</p>}
    {valid && value.status==="available" && value.summary && <PaceEvidenceView value={value.summary} language={language} reasonText={reason=>trialReason(reason,language)}/>}
    {!value ? <p>{failed?(zh?"暂时无法读取记录。":"Records are temporarily unavailable."):(zh?"正在读取本地记录…":"Reading local records…")}</p>
      : !valid || value.status!=="available" ? <p>{paceMessage(valid?value.reasonCode:"plan_context_changed",language)}</p>
      : !trials.length ? <p>{offset>0 || (value.summary?.counts.total??0)>0 ? (zh?"此页没有记录，请返回上一页。":"No records on this page; return to the previous page.") : archived ? (zh?"此旧环境没有保留的前瞻推算。":"No prospective scenarios are retained for this earlier reader.") : (zh?"暂无推算记录，后台试算已暂停。":"No saved scenarios. Background trials are paused.")}</p>
      : <><small>{zh?`本页：可比较 ${checked} 条，无法核验 ${skipped} 条，等待核验 ${trials.length-checked-skipped} 条。`:`This page: ${checked} comparable, ${skipped} unscorable, ${trials.length-checked-skipped} pending.`}</small><div className={styles.trialList}>{trials.map(trial=><Trial key={trial.trialId} value={trial} language={language} now={now} archived={archived}/>)}</div></>}
    <div className={styles.actions}><button disabled={loading} onClick={onReload}>{loading?(zh?"读取中…":"Reading…"):(zh?"重新读取记录":"Reload records")}</button><button disabled={loading||offset===0} onClick={()=>onPage(Math.max(0,offset-20))}>{zh?"上一页":"Previous"}</button><button disabled={loading||!valid||!value.hasMore||offset>=100000} onClick={()=>onPage(offset+20)}>{zh?"下一页":"Next"}</button></div>
  </div>;
}
export function PaceTrialHistory({contextId,sequence,language,read}: {contextId:string;sequence:number;language:Language;read:(context:string,offset:number)=>Promise<DesktopPaceTrialsEnvelope>}) {
  const store=useMemo(()=>createPaceTrialStore(contextId,read),[contextId,read]);
  const state=useSyncExternalStore(store.subscribe,store.getSnapshot,store.getSnapshot);
  useEffect(()=>{void store.reload();},[store,sequence]);
  useEffect(()=>{
    if(!state.open) return;
    const reload=()=>{if(document.visibilityState==="visible") void store.reload();};
    window.addEventListener("focus",reload);document.addEventListener("visibilitychange",reload);
    const timer=window.setInterval(reload,30000);
    return ()=>{window.clearInterval(timer);window.removeEventListener("focus",reload);document.removeEventListener("visibilitychange",reload);};
  },[store,state.open]);
  return <details className={styles.trialHistory} open={state.open} onToggle={event=>{const open=event.currentTarget.open;if(open!==state.open)store.setOpen(open);}}><summary>{language==="zh"?"过去的推算与实际":"Past scenarios and actual outcomes"}</summary>{state.open && <PaceTrialHistoryView {...state} context={contextId} language={language} onReload={()=>void store.reload()} onPage={store.setOffset}/>}</details>;
}
