import {renderToStaticMarkup} from "react-dom/server";
import {describe,expect,it,vi} from "vitest";
import type {DesktopPaceTrialsEnvelope,PaceTrialView} from "../../../../capacity-preview/src/status";
import {PaceTrialHistoryView,trialReason} from "./PaceTrialHistory";
import {createPaceTrialStore} from "./paceTrialStore";
const now=Date.parse("2026-09-26T12:00:00Z");
function trial():PaceTrialView {return {trialId:"private-trial",algorithmVersion:"recent-block-pace-experimental-v1",issuedAt:"2026-09-26T00:00:00Z",observedAt:"2026-09-26T00:00:00Z",forecastHorizon:"2026-09-26T06:00:00Z",limitId:"codex:weekly",windowMinutes:10080,balanceRange:[13,19],compatibilityUnverified:false,outcome:{verifierVersion:"pace-balance-outcome-v1",assessedAt:"2026-09-26T06:06:00Z",classification:"within_band",reasonCode:"observed_comparison",observedRange:[17,18],observedFrom:"2026-09-26T05:55:00Z",observedUntil:"2026-09-26T06:05:00Z",sampleCount:25,maximumGapSeconds:900,compatibilityUnverified:false}};}
function envelope():DesktopPaceTrialsEnvelope {return {historyContextId:"a",status:"available",reasonCode:"trials_available",offset:0,hasMore:false,recordingFailed:false,trials:[trial()],summary:null};}
function render(value:DesktopPaceTrialsEnvelope|null,extra:Partial<Parameters<typeof PaceTrialHistoryView>[0]>={}) {return renderToStaticMarkup(<PaceTrialHistoryView value={value} context="a" offset={0} failed={false} loading={false} language="zh" now={now} onPage={()=>undefined} onReload={()=>undefined} {...extra}/>);}
function deferred<T>(){let resolve!:(value:T)=>void;const promise=new Promise<T>(yes=>{resolve=yes;});return {promise,resolve};}
describe("prospective pace history",()=>{
  it("does not promise automatic recording or evaluation while experiments are suspended",()=>{
    const value=envelope();value.trials[0].outcome=null;
    for(const html of [render(value),render({...value,trials:[]})]) {
      expect(html).toContain("已暂停");expect(html).not.toContain("会自动开始");expect(html).not.toContain("下一次自动采集");
    }
  });
  it("compares frozen and subsequent ranges without raw codes or accuracy claims",()=>{
    const html=render(envelope());expect(html).toContain("13–19%");expect(html).toContain("17–18%");expect(html).toContain("落在原范围内");expect(html).toContain("可比较 1 条");expect(html).toContain("实验推算记录");
    const visibleText=html.replace(/<[^>]+>/g,"");
    for(const code of ["private-trial","codex:weekly","within_band","recent-block-pace-experimental-v1","pace-balance-outcome-v1"])expect(visibleText).not.toContain(code);
  });
  it("distinguishes missing or inconclusive data from observed zero or a hit",()=>{
    const value=envelope();value.trials[0].outcome={...value.trials[0].outcome!,classification:"unscorable",reasonCode:"target_samples_missing",observedRange:null,observedFrom:null,observedUntil:null};
    const html=render(value);expect(html).toContain("无法核验 1 条");expect(html).toContain("不计为命中");expect(html).not.toContain("0%");expect(html).not.toContain("落在原范围内");expect(trialReason("boundary_uncertain","en")).toContain("inconclusive");
  });
  it("keeps expired pending distinct from prematurely scored outcomes",()=>{
    const value=envelope();value.trials[0].outcome=null;expect(render(value)).toContain("目标已到");expect(render(value,{now:now-86400000})).toContain("显示已保存的推算");
  });
  it("keeps earlier-reader pending records separate without promising current refresh will score them",()=>{
    const value=envelope();value.trials[0].outcome=null;const html=render(value,{archived:true});
    expect(html).toContain("所选旧环境");expect(html).toContain("该旧环境未完成核验");expect(html).not.toContain("下一次自动采集");
    expect(render({...value,trials:[]},{archived:true})).toContain("此旧环境没有保留");
  });
  it("marks retained reads and recorder failure but removes unauthorized data",()=>{
    const value=envelope();value.recordingFailed=true;expect(render(value,{failed:true})).toContain("暂保留本页上次记录");expect(render(value)).toContain("额度监控不受影响");
    expect(render({...value,historyContextId:"b"})).not.toContain("13–19%");expect(render({...value,status:"unavailable",reasonCode:"history_keychain_denied",trials:[]})).toContain("本地历史密钥尚未授权");
  });
  it("explains empty prospective history instead of manufacturing earlier predictions",()=>{expect(render({...envelope(),trials:[]})).toContain("暂无推算记录");expect(render(envelope(),{language:"en"})).toContain("Within the original band");});
  it("does not read while closed and closing suppresses queued work",async()=>{
    const pending=deferred<DesktopPaceTrialsEnvelope>();const read=vi.fn().mockReturnValue(pending.promise);const store=createPaceTrialStore("a",read);const dispose=store.subscribe(()=>undefined);
    await store.reload();expect(read).not.toHaveBeenCalled();store.setOpen(true);const task=store.reload();store.setOpen(false);pending.resolve(envelope());await task;expect(read).toHaveBeenCalledTimes(1);expect(store.getSnapshot().value).toBeNull();dispose();
  });
  it("discards a delayed previous-page response and loads the selected page",async()=>{
    const pending=deferred<DesktopPaceTrialsEnvelope>();const read=vi.fn().mockReturnValueOnce(pending.promise).mockResolvedValue({...envelope(),offset:20,trials:[]});const store=createPaceTrialStore("a",read);const dispose=store.subscribe(()=>undefined);
    store.setOpen(true);store.setOffset(20);const task=store.reload();pending.resolve(envelope());await task;expect(store.getSnapshot().value?.offset).toBe(20);expect(store.getSnapshot().value?.trials).toEqual([]);dispose();
  });
  it("retains failures but clears mismatched context and unavailable responses",async()=>{
    const read=vi.fn().mockResolvedValue(envelope());const store=createPaceTrialStore("a",read);const dispose=store.subscribe(()=>undefined);store.setOpen(true);await vi.waitFor(()=>expect(store.getSnapshot().loading).toBe(false));
    read.mockRejectedValueOnce(new Error("synthetic private error"));await store.reload();expect(store.getSnapshot().failed).toBe(true);expect(store.getSnapshot().value?.trials.length).toBe(1);
    read.mockResolvedValueOnce({...envelope(),historyContextId:"b"});await store.reload();expect(store.getSnapshot().value).toBeNull();
    read.mockResolvedValueOnce({...envelope(),status:"unavailable",trials:[]});await store.reload();expect(store.getSnapshot().value?.trials).toEqual([]);dispose();
  });
});
