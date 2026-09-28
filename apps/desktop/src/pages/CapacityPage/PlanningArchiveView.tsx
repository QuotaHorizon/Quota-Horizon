import {useEffect, useMemo, useState, useSyncExternalStore} from "react";
import type {ReactNode} from "react";
import type {DesktopDemandPlan, DesktopPlanningArchiveEnvelope, PlanningArchiveEntry, PlanningArchiveQuery, PlanningArchiveRef} from "../../../../capacity-preview/src/status";
import type {Language} from "../../i18n";
import {archiveQueryKey, createPlanningArchiveStore, type PlanningArchiveReader} from "./planningArchiveStore";
import {demandMessage, planTime} from "./demandPlanModel";
import {ActivityQuotaView} from "./ActivityQuotaView";
import {PaceTrialHistoryView} from "./PaceTrialHistory";
import styles from "./PlanningArchive.module.less";

interface Props {contextId: string; sequence: number; language: Language; read: PlanningArchiveReader; onUse: (plan: DesktopDemandPlan) => void; canUse: boolean}
const number = (value: number, language: Language) => value.toLocaleString(language === "zh" ? "zh-CN" : "en-US", {maximumFractionDigits: 1});
export function archiveReaderLabel(entry: PlanningArchiveEntry, language: Language) {
  const zh = language === "zh";
  const platform = ({macos: "macOS", windows: "Windows", linux: "Linux"} as Record<string, string>)[entry.platform] ?? (zh ? "其他平台" : "Other platform");
  const boundary = entry.boundary === "native" ? (zh ? "本机读取器" : "Local reader") : (zh ? "其他读取环境" : "Other reader environment");
  // Do not surface arbitrary metadata or hashes as a product label.
  const version = entry.codexVersion?.match(/^\d+(?:\.\d+){1,3}(?:[-+][a-zA-Z0-9.-]{1,32})?$/u)?.[0];
  return `${platform} · ${boundary}${version ? ` · Codex ${version}` : ""}`;
}

function useArchive(props: Props, query: PlanningArchiveQuery) {
  const key = archiveQueryKey(query);
  const store = useMemo(() => createPlanningArchiveStore(props.contextId, query, props.read), [props.contextId, key, props.read]);
  const state = useSyncExternalStore(store.subscribe, store.getSnapshot, store.getSnapshot);
  useEffect(() => {void store.reload();}, [store, props.sequence]);
  useEffect(() => {
    const refresh = () => {if (document.visibilityState === "visible") void store.reload();};
    window.addEventListener("focus", refresh); document.addEventListener("visibilitychange", refresh);
    return () => {window.removeEventListener("focus", refresh); document.removeEventListener("visibilitychange", refresh);};
  }, [store]);
  return {...state, reload: () => void store.reload()};
}

export function ArchiveReadState({value, failed, loading, language, onRetry, children}: {value: DesktopPlanningArchiveEnvelope | null; failed: boolean; loading: boolean; language: Language; onRetry: () => void; children?: ReactNode}) {
  const zh = language === "zh";
  return <>
    <div className={styles.refresh}><button disabled={loading} onClick={onRetry}>{loading ? (zh ? "读取中…" : "Reading…") : (zh ? "重新读取" : "Read again")}</button></div>
    {failed && <p role="status" className={styles.notice}>{zh ? "读取失败；保留的是上次结果，暂不能载入为新计划。请重试。" : "Could not read the archive. Retained content is the previous result and cannot be reused until a successful read."}</p>}
    {!value && !failed && <p>{zh ? "正在读取本机旧记录…" : "Reading local archives…"}</p>}
    {value && value.status !== "available" && <p>{value.status === "missing" ? (zh ? "该旧记录已不可用，或已成为当前读取环境。请返回重新读取列表。" : "This archive is unavailable or is now the current reader. Return and refresh the list.") : value.status === "invalid_request" ? (zh ? "无法读取所选记录，请返回重新选择。" : "Could not read this selection. Return and choose it again.") : demandMessage(value.reasonCode, language)}</p>}
    {value?.status === "available" && children}
  </>;
}

function Pagination({offset, hasMore, disabled, language, change}: {offset: number; hasMore: boolean; disabled: boolean; language: Language; change: (offset: number) => void}) {
  if (!offset && !hasMore) return null;
  const zh = language === "zh";
  return <div className={styles.pages}><button disabled={disabled || offset === 0} onClick={() => change(Math.max(0, offset - 20))}>{zh ? "上一页" : "Previous"}</button><span>{zh ? `第 ${offset / 20 + 1} 页 · 每页最多 20 条` : `Page ${offset / 20 + 1} · up to 20 per page`}</span><button disabled={disabled || !hasMore} onClick={() => change(offset + 20)}>{zh ? "下一页" : "Next"}</button></div>;
}

export function PlanningArchiveView(props: Props) {
  const [selected, setSelected] = useState<PlanningArchiveEntry | null>(null);
  const [offset, setOffset] = useState(0);
  const zh = props.language === "zh";
  // Details and list have independent lifetimes. Hidden views do not reread.
  return <div className={styles.archive}>
    <p className={styles.notice}>{zh ? "这里是当前账号在旧读取环境中的计划与记录。更新 Codex 不会删除它们；查看不会切换账号、恢复计时或合并统计。" : "These are this account’s plans and records from earlier readers. Codex updates do not delete them. Viewing does not switch accounts, resume timers or merge statistics."}</p>
    {selected ? <><button onClick={() => setSelected(null)}>{zh ? "← 返回旧环境列表" : "← Back to earlier readers"}</button><h4>{archiveReaderLabel(selected, props.language)}</h4><ArchiveDetails key={archiveQueryKey({kind: "details", source: selected.source, offset: 0})} {...props} source={selected.source}/></>
      : <ArchiveList {...props} offset={offset} onPage={setOffset} onSelect={setSelected}/>}
  </div>;
}

function ArchiveList(props: Props & {offset: number; onPage: (n: number) => void; onSelect: (entry: PlanningArchiveEntry) => void}) {
  const state = useArchive(props, {kind: "list", offset: props.offset});
  const list = state.value?.result?.kind === "list" ? state.value.result : null;
  const zh = props.language === "zh";
  return <ArchiveReadState {...state} language={props.language} onRetry={state.reload}>
    {list && <>{!list.entries.length && <p>{zh ? "当前账号没有其他读取环境的计划、使用记录或推算。未登录或未授权时不会显示其他账号的数据。" : "This account has no plans, activity or scenarios in earlier readers. Other accounts are never shown here."}</p>}
      <ul className={styles.list}>{list.entries.map(entry => <li key={`${entry.source.kind}:${entry.source.id}`}><div><strong>{archiveReaderLabel(entry, props.language)}</strong><span>{zh ? `最近保存于 ${planTime(entry.lastRecordedAt, props.language)}` : `Last saved ${planTime(entry.lastRecordedAt, props.language)}`}</span><small>{zh ? `${entry.planRevisions} 个计划版本 · ${entry.recordCount} 条使用记录 · ${entry.trialCount} 条推算` : `${entry.planRevisions} plan revisions · ${entry.recordCount} activity records · ${entry.trialCount} scenarios`}</small></div><button disabled={state.failed || state.loading} onClick={() => props.onSelect(entry)}>{zh ? "查看" : "View"}</button></li>)}</ul>
      <Pagination offset={props.offset} hasMore={list.hasMore} disabled={state.loading} language={props.language} change={props.onPage}/></>}
  </ArchiveReadState>;
}

function ArchiveDetails(props: Props & {source: PlanningArchiveRef}) {
  const [offset, setOffset] = useState(0), [evidenceId, setEvidenceId] = useState<string | null>(null);
  const [paceOpen, setPaceOpen] = useState(false);
  const state = useArchive(props, {kind: "details", source: props.source, offset});
  const details = state.value?.result?.kind === "details" ? state.value.result : null;
  const zh = props.language === "zh";
  return <ArchiveReadState {...state} language={props.language} onRetry={state.reload}>
    {details && <>
      <button aria-expanded={paceOpen} disabled={!paceOpen && (state.failed || state.loading)} onClick={()=>setPaceOpen(open=>!open)}>{paceOpen?(zh?"收起旧推算":"Hide earlier scenarios"):(zh?"查看此环境的推算与核验":"View scenarios and checks from this reader")}</button>
      {paceOpen && <ArchivePace {...props}/>}
      <h4>{zh ? "最近 5 个计划版本" : "Latest 5 plan revisions"}</h4>
      {!details.plans.length && <p>{zh ? "此环境没有保存的计划。" : "No plans were saved in this reader."}</p>}
      <ul className={styles.list}>{details.plans.map(plan => <li key={plan.revision}><div><strong>{plan.demand.demandKind === "active_hours" ? (zh ? `计划主动使用 ${number(plan.demand.plannedCodexActiveHours ?? 0, props.language)} 小时` : `${number(plan.demand.plannedCodexActiveHours ?? 0, props.language)} planned active hours`) : (zh ? "维持近期使用节奏" : "Maintain my recent pace")}</strong><span>{zh ? "截至 " : "Until "}{planTime(plan.demand.horizonEnd, props.language)}</span><small>{zh ? `第 ${plan.revision} 版 · ` : `Revision ${plan.revision} · `}{plan.enabled ? (zh ? "原计划启用" : "Was enabled") : (zh ? "原计划暂停" : "Was paused")} · {planTime(plan.createdAt, props.language)}</small></div><button disabled={!props.canUse || state.failed || state.loading} onClick={() => props.onUse(plan)}>{zh ? "载入为新计划草稿" : "Use as a new draft"}</button></li>)}</ul>
      <p className={styles.muted}>{zh ? "只复制需求数值，需核对剩余时长和截止时间后保存。不会搬移旧记录，也不会把旧消耗用于当前预测。已有编辑未完成时请先保存或取消。" : "Only demand values are copied. Review remaining hours and the deadline before saving. Records are not moved or used in current forecasts. Finish or cancel any open editor first."}</p>
      {details.draft && <div className={styles.draft}><strong>{zh ? "旧计时草稿 · 未计入使用记录" : "Earlier timer draft · not recorded usage"}</strong><p>{planTime(details.draft.startedAt, props.language)} → {planTime(details.draft.lastCheckpoint, props.language)}</p><p>{zh ? `截至最后检查点，计时参考 ${number(details.draft.suggestedSeconds / 60, props.language)} 分钟；没有补计后续时间，也没有自动恢复。` : `${number(details.draft.suggestedSeconds / 60, props.language)} suggested minutes at its last checkpoint. No later time is added and the timer is not resumed.`}</p></div>}
      <h4>{zh ? "旧使用记录 · 只读" : "Earlier activity · read-only"}</h4>
      {!details.records.length && <p>{zh ? "这一页没有确认的使用记录。" : "No confirmed activity on this page."}</p>}
      <ul className={styles.list}>{details.records.map(record => <li key={record.observationId} className={styles.record}><div><strong>{number(record.observation.durationSeconds / 60, props.language)} {zh ? "分钟" : "min"}</strong><span>{planTime(record.observation.startedAt, props.language)} → {planTime(record.observation.endedAt, props.language)}</span><small>{record.source === "user_timer" ? (zh ? "计时后确认" : "Confirmed timer") : (zh ? "手动确认" : "Manually confirmed")} · {record.included ? (zh ? "原环境已纳入" : "Included in original reader") : (zh ? "原环境已排除" : "Excluded in original reader")}</small></div><button disabled={state.loading || state.failed} onClick={() => setEvidenceId(id => id === record.observationId ? null : record.observationId)}>{evidenceId === record.observationId ? (zh ? "收起同期额度" : "Hide quota") : (zh ? "原环境同期额度" : "Quota in original reader")}</button>
        {evidenceId === record.observationId && <div className={styles.evidence}><ArchiveQuota key={record.observationId} {...props} observationId={record.observationId}/></div>}
      </li>)}</ul>
      <Pagination offset={offset} hasMore={details.hasMore} disabled={state.loading} language={props.language} change={next => {setEvidenceId(null); setOffset(next);}}/>
    </>}
  </ArchiveReadState>;
}

function ArchivePace(props: Props & {source: PlanningArchiveRef}) {
  const [offset,setOffset]=useState(0);
  const state=useArchive(props,{kind:"pace",source:props.source,offset});
  const data=state.value?.status==="available" && state.value.result?.kind==="pace" ? state.value.result : null;
  return data ? <PaceTrialHistoryView archived value={{historyContextId:props.contextId,status:"available",reasonCode:"trials_available",offset,hasMore:data.hasMore,recordingFailed:false,trials:data.trials,summary:null}} context={props.contextId} offset={offset} failed={state.failed} loading={state.loading} language={props.language} onReload={state.reload} onPage={setOffset}/>
    : <ArchiveReadState {...state} language={props.language} onRetry={state.reload}/>;
}

function ArchiveQuota(props: Props & {source: PlanningArchiveRef; observationId: string}) {
  const state = useArchive(props, {kind: "quota", source: props.source, observationId: props.observationId});
  const data = state.value?.status === "available" && state.value.result?.kind === "quota" ? state.value.result : null;
  return data ? <ActivityQuotaView archived value={{schemaVersion: "1.0", historyContextId: props.contextId, observationId: props.observationId, status: "available", reasonCode: "activity_evidence_available", record: data.record, comparison: data.comparison, generatedAt: state.value!.generatedAt}} language={props.language} failed={state.failed} loading={state.loading} onRetry={state.reload}/>
    : <ArchiveReadState {...state} language={props.language} onRetry={state.reload}/>;
}
