import { useEffect, useRef, useState, type ReactNode } from "react";
import { Timer } from "lucide-react";
import type { ActiveTimeAction, ActiveTimeInput, ActiveTimer, DesktopActiveTimeEnvelope, DesktopActiveTimeUpdate } from "../../../../capacity-preview/src/status";
import type { Language } from "../../i18n";
import { activityFromForm, activityMessage, localActivityInput, suggestedTimerSeconds, timerDisplay } from "./activeTimeModel";
import { planTime } from "./demandPlanModel";
import styles from "./CapacityActiveTime.module.less";

interface Props {
  contextId: string; value: DesktopActiveTimeEnvelope | null; language: Language;
  busy?: boolean; failed?: boolean; compact?: boolean; now?: number;
  onSave: (request: DesktopActiveTimeUpdate) => Promise<boolean>; onRetry: () => void; onOpen?: () => void;
  renderEvidence?: (observationId: string, revision: number) => ReactNode;
}
interface Draft { id: string; timerRevision?: number; start: string; end: string; minutes: string; original?: ActiveTimeInput; originalMinutes?: string }
function timerDraft(timer: ActiveTimer): Draft {
  const original = { startedAt:timer.startedAt,endedAt:timer.endedAt ?? timer.updatedAt,durationSeconds:timer.suggestedSeconds };
  const minutes = (timer.suggestedSeconds/60).toFixed(2);
  return {id:timer.timerId,timerRevision:timer.revision,start:localActivityInput(original.startedAt),end:localActivityInput(original.endedAt),minutes,original,originalMinutes:minutes};
}
export function CapacityActiveTimeView({contextId,value,language,busy=false,failed=false,compact=false,now:fixedNow,onSave,onRetry,onOpen,renderEvidence}:Props) {
  const zh=language==="zh"; const [clock,setClock]=useState(Date.now);
  const durationText=(seconds:number) => {
    const unit=seconds>=3600 ? 3600 : seconds>=60 ? 60 : 1;
    return {value:(seconds/unit).toLocaleString(zh?"zh-CN":"en-US",{maximumFractionDigits:1}),label:unit===3600?(zh?"小时":"hours"):unit===60?(zh?"分钟":"min"):(zh?"秒":"sec")};
  };
  const now=fixedNow ?? clock;
  const [draft,setDraft]=useState<Draft|null>(null);
  const [reviewRequested,setReviewRequested]=useState<string|null>(null);
  const [discardId,setDiscardId]=useState<string|null>(null);
  const [evidenceId,setEvidenceId]=useState<string|null>(null);
  const startId=useRef<string|null>(null);
  const timer=value?.timer ?? null;
  const usable=!!value && value.historyContextId===contextId && value.status!=="unavailable";
  const disabled=busy || failed || !usable;
  useEffect(()=>{
    if (fixedNow!==undefined) return;
    setClock(Date.now());
    const tick=window.setInterval(()=>setClock(Date.now()),timer?.state==="running"?1000:30_000);
    return ()=>window.clearInterval(tick);
  },[fixedNow,timer?.state]);
  useEffect(()=>{
    if (!usable) {setDraft(null);setDiscardId(null);setReviewRequested(null);setEvidenceId(null);}
    else if (reviewRequested && timer?.timerId===reviewRequested && timer.state==="review" && !compact) {
      setDraft(timerDraft(timer));setReviewRequested(null);
    }
  },[usable,reviewRequested,timer,compact]);
  const act=(action:ActiveTimeAction)=>onSave({historyContextId:contextId,action});
  async function start() {
    startId.current ??= crypto.randomUUID();
    if (await act({kind:"start",timerId:startId.current})) startId.current=null;
  }
  async function timerAction(kind:"pause"|"resume"|"finish"|"discard") {
    if (!timer) return;
    if (await act({kind,timerId:timer.timerId,expectedRevision:timer.revision})) {
      setDiscardId(null);
      if (kind==="finish") setReviewRequested(timer.timerId);
      if (kind==="discard") setDraft(null);
    }
  }
  function manual() {
    const currentNow=fixedNow ?? Date.now();
    setClock(currentNow);
    const end=new Date(Math.floor(currentNow/1000)*1000).toISOString();
    setDraft({id:crypto.randomUUID(),start:localActivityInput(new Date(Date.parse(end)-30*60_000).toISOString()),end:localActivityInput(end),minutes:"30"});
  }
  const draftConflict=!!draft && (draft.timerRevision!==undefined
    ? timer?.timerId!==draft.id || timer.revision!==draft.timerRevision || timer.state!=="review"
    : !!timer);
  const input=draft ? activityFromForm(draft.start,draft.end,draft.minutes,now,draft.minutes===draft.originalMinutes ? draft.original?.durationSeconds : undefined,draft.original) : null;
  const badStatus=value && !["available","updated","unavailable"].includes(value.status);
  const title=zh?"实际使用记录":"Actual use records";
  const total=durationText(value?.includedSeconds30Days ?? 0);
  return <section className={`${styles.panel} ${compact?styles.compact:""}`} aria-label={title}>
    <div className={styles.heading}><h3><Timer size={compact?14:17}/>{title}</h3>{compact && onOpen && <button onClick={onOpen}>{zh?"查看":"View"}</button>}</div>
    {!value && !failed ? <p className={styles.muted}>{zh?"读取本地记录…":"Reading local records…"}</p> : !usable ? <p className={styles.muted}>{activityMessage(value?.reasonCode ?? "",language)}</p> : <>
      <div className={styles.summary}><strong>{total.value}<small> {total.label}</small></strong><span>{zh?`最近 30 天已确认 · ${value.includedCount30Days} 条`:`Confirmed in the last 30 days · ${value.includedCount30Days} ${value.includedCount30Days===1?"record":"records"}`}</span></div>
      {timer ? <div className={styles.timer}>
        <div className={styles.timerValue}><strong>{timerDisplay(suggestedTimerSeconds(timer,now))}</strong><span>{timer.state==="running" ? (zh?"计时参考 · 未计入记录":"Timer suggestion · not recorded") : timer.state==="paused" ? (zh?"已暂停 · 未计入记录":"Paused · not recorded") : (zh?"待核对 · 未计入记录":"Needs review · not recorded")}</span></div>
        {timer.interrupted && <p className={styles.muted}>{zh?"计时已中断，仅保留上次记录点。请核对时间和实际分钟，不自动补算缺失时段。":"The timer was interrupted. Only its last checkpoint is retained. Review the interval and actual minutes; no missing time is added automatically."}</p>}
        <div className={styles.actions}>
          {timer.state==="running" && <button disabled={disabled} onClick={()=>void timerAction("pause")}>{zh?"暂停":"Pause"}</button>}
          {timer.state==="paused" && <button disabled={disabled} onClick={()=>void timerAction("resume")}>{zh?"继续计时":"Resume"}</button>}
          {timer.state!=="review" && <button disabled={disabled} onClick={()=>void timerAction("finish")}>{zh?"结束并核对":"Finish & review"}</button>}
          {timer.state==="review" && <button disabled={disabled} onClick={()=>compact ? onOpen?.() : setDraft(timerDraft(timer))}>{zh?"核对并保存":"Review & save"}</button>}
          {discardId===timer.timerId ? <><button disabled={disabled} onClick={()=>void timerAction("discard")}>{zh?"确认放弃这次计时":"Confirm discard"}</button><button onClick={()=>setDiscardId(null)}>{zh?"返回":"Back"}</button></> : <button disabled={disabled} onClick={()=>setDiscardId(timer.timerId)}>{zh?"放弃计时":"Discard timer"}</button>}
        </div>
      </div> : <div className={styles.actions}><button disabled={disabled || !!draft} onClick={()=>void start()}>{zh?"开始计时":"Start timer"}</button>{!compact && <button disabled={disabled || !!draft} onClick={manual}>{zh?"补录使用":"Add past use"}</button>}</div>}
      {!compact && draft && <form className={styles.form} onSubmit={async(event)=>{
        event.preventDefault(); if (!input || disabled || draftConflict) return;
        const action:ActiveTimeAction=draft.timerRevision===undefined ? {kind:"record",observationId:draft.id,observation:input} : {kind:"confirm",timerId:draft.id,expectedRevision:draft.timerRevision,observation:input};
        if (await act(action)) setDraft(null);
      }}>
        <div className={styles.formTitle}><strong>{draft.timerRevision===undefined ? (zh?"补录实际使用":"Record actual use") : (zh?"核对本次使用":"Review this use")}</strong><span>{Intl.DateTimeFormat().resolvedOptions().timeZone}</span></div>
        <label>{zh?"开始时间":"Start time"}<input required type="datetime-local" step="1" value={draft.start} disabled={busy} onChange={(e)=>setDraft({...draft,start:e.target.value})}/></label>
        <label>{zh?"结束时间":"End time"}<input required type="datetime-local" step="1" value={draft.end} disabled={busy} onChange={(e)=>setDraft({...draft,end:e.target.value})}/></label>
        <label>{zh?"实际使用分钟":"Actual minutes"}<input required type="number" min="0.01" max="1440" step="0.01" value={draft.minutes} disabled={busy} onChange={(e)=>setDraft({...draft,minutes:e.target.value})}/></label>
        <p className={styles.muted}>{zh?"扣除挂机、离开和未使用 Codex 的时间。确认后才计入记录，不会自动扣减计划小时。":"Exclude idle time, breaks and time not using Codex. Only confirmed minutes enter records; planned hours are not automatically reduced."}</p>
        {draftConflict && <p className={styles.notice}>{zh?"当前计时已在另一处改变，保留输入供你核对，请先读取最新状态。":"This timer changed in another view. Your input is retained for review; load the latest state first."} <button type="button" onClick={()=>{setDraft(timer?.state==="review"?timerDraft(timer):null);onRetry();}}>{zh?"读取最新状态":"Load latest state"}</button></p>}
        {!input && !draftConflict && <p className={styles.notice}>{zh?"请填写有效的本地时间和分钟数。时段最多 7 天，实际使用最多 24 小时且不超过时段；未来时间和不明确的夏令时切换时刻不能保存。":"Use valid local dates and minutes. The interval may span up to 7 days, with up to 24 hours of actual use within it. Future or ambiguous daylight-saving times cannot be saved."}</p>}
        <div className={styles.actions}><button type="submit" disabled={disabled || !input || draftConflict}>{busy?(zh?"保存中…":"Saving…"):(zh?"确认实际分钟并保存":"Confirm minutes & save")}</button><button type="button" disabled={busy} onClick={()=>setDraft(null)}>{zh?"稍后再填":"Later"}</button></div>
      </form>}
      <p className={styles.muted}>{zh?"仅主动计时或手动录入，不监听键盘、窗口或对话。记录是后续校准的输入，当前不生成可用小时预测。":"Explicit timers or manual entries only. No keyboard, window or conversation monitoring. These are calibration inputs, not an available-hours forecast."}</p>
      {value.hasOtherEnvironmentRecords && <p className={styles.muted}>{zh?"其他读取环境下的记录仍保留在本机，未自动合并到此处。可在主窗口“我的工作需求”内打开旧环境记录。":"Records from earlier readers are retained locally and are not merged automatically. Open earlier-reader records under My work demand in the main window."}</p>}
      {!compact && value.observations.length>0 && <details className={styles.records} onToggle={event=>{if (!event.currentTarget.open) setEvidenceId(null);}}><summary>{zh?"最近记录（最多 20 条）":"Recent records (up to 20)"}</summary><ol>{value.observations.map((row)=><li key={row.observationId} className={!row.included?styles.excluded:""}>
        <div><strong>{durationText(row.observation.durationSeconds).value} {durationText(row.observation.durationSeconds).label}</strong><span>{row.source==="user_timer"?(zh?"计时后确认":"Timer, confirmed"):(zh?"手动确认":"Manually confirmed")}{!row.included && (zh?" · 已排除":" · Excluded")}</span><small>{planTime(row.observation.startedAt,language)} → {planTime(row.observation.endedAt,language)}</small></div>
        <div className={styles.recordActions}>{renderEvidence && <button aria-expanded={evidenceId===row.observationId} onClick={()=>setEvidenceId(evidenceId===row.observationId?null:row.observationId)}>{evidenceId===row.observationId?(zh?"收起同期额度":"Hide quota"):(zh?"同期额度":"Quota in interval")}</button>}<button disabled={disabled} onClick={()=>void act({kind:"set_included",observationId:row.observationId,expectedRevision:row.revision,included:!row.included})}>{row.included?(zh?"排除":"Exclude"):(zh?"重新纳入":"Include again")}</button></div>
        {evidenceId===row.observationId && renderEvidence && <div className={styles.evidence}>{renderEvidence(row.observationId,row.revision)}</div>}
      </li>)}</ol></details>}
    </>}
    {(failed || badStatus) && <div className={styles.notice} role="status"><span>{activityMessage(failed?"activity_read_failed":value?.reasonCode ?? "",language)}</span><button disabled={busy} onClick={onRetry}>{zh?"重试读取":"Retry read"}</button></div>}
  </section>;
}
