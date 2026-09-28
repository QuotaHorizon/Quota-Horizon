import {renderToStaticMarkup} from "react-dom/server";
import {describe,expect,it,vi} from "vitest";
import type {DesktopActiveTimeQuotaEnvelope,DesktopActiveTimeEnvelope} from "../../../../capacity-preview/src/status";
import {ActivityQuotaView,activityQuotaReason} from "./ActivityQuotaView";
import {createActivityQuotaStore} from "./activityQuotaStore";
import {CapacityActiveTimeView} from "./CapacityActiveTimeView";

function evidence():DesktopActiveTimeQuotaEnvelope {
  return {schemaVersion:"1.0",historyContextId:"scope-a",observationId:"observation-a",status:"available",reasonCode:"activity_evidence_available",generatedAt:"2026-09-26T04:00:00Z",
    record:{observationId:"observation-a",source:"user_reported",quality:"user_confirmed",observation:{startedAt:"2026-09-26T02:00:00Z",endedAt:"2026-09-26T03:00:00Z",durationSeconds:1200},included:true,revision:1,createdAt:"2026-09-26T04:00:00Z"},
    comparison:{algorithmVersion:"activity-quota-comparison-v1",queryTruncated:false,windows:[{limitId:"codex:weekly",windowMinutes:10080,reasonCode:"comparable",sampleCount:4,firstSnapshotId:"private-start",lastSnapshotId:"private-end",observedFrom:"2026-09-26T02:10:00Z",observedUntil:"2026-09-26T02:50:00Z",firstRemainingPercent:40.5,lastRemainingPercent:37.5,observedDecreasePercent:3,coveragePercent:66.6,leadingGapSeconds:600,trailingGapSeconds:600,maximumGapSeconds:900,compatibilityUnverified:false}]}};
}
function render(value:DesktopActiveTimeQuotaEnvelope|null,options:Partial<Parameters<typeof ActivityQuotaView>[0]>={}) {return renderToStaticMarkup(<ActivityQuotaView value={value} language="zh" onRetry={()=>undefined} {...options}/>);}
function deferred<T>() {let resolve!:(value:T)=>void;const promise=new Promise<T>(yes=>{resolve=yes;});return {promise,resolve};}
describe("activity quota evidence",()=>{
  it("shows original observation coverage without converting minutes to hourly consumption",()=>{
    const html=render(evidence());expect(html).toContain("40.5% → 37.5%");expect(html).toContain("观测下降 3 个百分点");expect(html).toContain("覆盖记录时段 66.6%");expect(html).toContain("起始未覆盖 10 分钟");expect(html).toContain("包含暂停期间");expect(html).toContain("其他设备");
    for (const raw of ["private-start","private-end","activity-quota-comparison-v1","comparable","codex:weekly"]) expect(html).not.toContain(raw);
  });
  it("does not interpret flat captures as proof of zero use",()=>{
    const v=evidence();v.comparison!.windows[0].observedDecreasePercent=0;const html=render(v);expect(html).toContain("采样期间余额未见变化");expect(html).not.toContain("下降 0");
  });
  it("explains intermediate rises without making a forced-reset claim",()=>{
    const v=evidence();Object.assign(v.comparison!.windows[0],{reasonCode:"balance_increased",observedDecreasePercent:null});const html=render(v);expect(html).toContain("期间额度回升");expect(html).toContain("暂不汇总消耗");expect(html).not.toContain("观测下降 3");
  });
  it("keeps missing windows and empty history distinct from zero",()=>{
    const v=evidence();v.comparison!.windows=[];expect(render(v)).toContain("该时段没有额度记录");expect(render(v)).not.toContain("0%");
    expect(render(evidence())).not.toContain("5 小时额度");
  });
  it("does not render ambiguous endpoints as a clean comparison",()=>{
    for (const reason of ["window_changed","conflicting_samples","incompatible_source","query_truncated"]) {
      const v=evidence();Object.assign(v.comparison!.windows[0],{reasonCode:reason,observedDecreasePercent:null});const html=render(v);expect(html).not.toContain("40.5% → 37.5%");expect(html).not.toContain(reason);
    }
    expect(activityQuotaReason("unexpected_internal_code","zh")).not.toContain("unexpected_internal_code");
  });
  it("shows exclusions, unverified compatibility and prior-result read failures",()=>{
    const v=evidence();v.record!.included=false;v.comparison!.windows[0].compatibilityUnverified=true;
    const html=render(v,{failed:true});expect(html).toContain("此记录已排除");expect(html).toContain("兼容性待验证");expect(html).toContain("上次结果");
    expect(render(v,{language:"en"})).toContain("including pauses and use on other devices");
  });
  it("does not load evidence until a record is explicitly expanded",()=>{
    const v=evidence();const renderEvidence=vi.fn(()=>null);const list:DesktopActiveTimeEnvelope={schemaVersion:"1.0",historyContextId:"scope-a",status:"available",reasonCode:"activity_available",revision:1,timer:null,observations:[v.record!],includedCount30Days:1,includedSeconds30Days:1200,hasOtherEnvironmentRecords:false,generatedAt:v.generatedAt};
    const html=renderToStaticMarkup(<CapacityActiveTimeView contextId="scope-a" value={list} language="zh" now={Date.parse(v.generatedAt)} onSave={async()=>true} onRetry={()=>undefined} renderEvidence={renderEvidence}/>);
    expect(html).toContain("同期额度");expect(renderEvidence).not.toHaveBeenCalled();
  });
  it("coalesces read requests and follows an update arriving during the old read",async()=>{
    const pending=deferred<DesktopActiveTimeQuotaEnvelope>();const next=evidence();next.record!.revision=2;const read=vi.fn().mockImplementationOnce(()=>pending.promise).mockResolvedValue(next);
    const s=createActivityQuotaStore("scope-a","observation-a",read);const first=s.reload();void s.reload();pending.resolve(evidence());await first;
    expect(read).toHaveBeenCalledTimes(2);expect(s.getSnapshot().value?.record?.revision).toBe(2);expect(s.getSnapshot().loading).toBe(false);
  });
  it("retains prior facts after failure but clears them on binding loss",async()=>{
    const denied={...evidence(),status:"unavailable",reasonCode:"history_keychain_denied",record:null,comparison:null};
    const read=vi.fn().mockResolvedValueOnce(evidence()).mockRejectedValueOnce(new Error("private error")).mockResolvedValueOnce(denied);
    const s=createActivityQuotaStore("scope-a","observation-a",read);await s.reload();await s.reload();expect(s.getSnapshot().failed).toBe(true);expect(s.getSnapshot().value?.comparison?.windows.length).toBe(1);
    await s.reload();expect(s.getSnapshot().value?.comparison).toBeNull();expect(s.getSnapshot().failed).toBe(false);
  });
  it("rejects mismatched context and record IDs, including delayed old responses",async()=>{
    for (const value of [{...evidence(),historyContextId:"scope-b"},{...evidence(),observationId:"observation-b"}]) {
      const s=createActivityQuotaStore("scope-a","observation-a",async()=>value);await s.reload();expect(s.getSnapshot().failed).toBe(true);expect(s.getSnapshot().value).toBeNull();
    }
    const pending=deferred<DesktopActiveTimeQuotaEnvelope>();const old=createActivityQuotaStore("scope-a","observation-a",()=>pending.promise);const newValue={...evidence(),historyContextId:"scope-b",observationId:"observation-b"};
    const next=createActivityQuotaStore("scope-b","observation-b",async()=>newValue);const task=old.reload();await next.reload();pending.resolve(evidence());await task;expect(next.getSnapshot().value?.historyContextId).toBe("scope-b");
  });
});
