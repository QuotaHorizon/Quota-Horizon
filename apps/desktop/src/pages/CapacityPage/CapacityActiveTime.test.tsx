import {renderToStaticMarkup} from "react-dom/server";
import {describe,expect,it,vi} from "vitest";
import type {ActiveTimer,DesktopActiveTimeEnvelope,DesktopActiveTimeUpdate} from "../../../../capacity-preview/src/status";
import {acceptActiveTime,activityFromForm,activityMessage,localActivityInput,suggestedTimerSeconds,timerDisplay} from "./activeTimeModel";
import {createActiveTimeStore} from "./activeTimeStore";
import {CapacityActiveTimeView} from "./CapacityActiveTimeView";

const now=Date.parse("2026-09-26T04:00:00Z");
const start="2026-09-26T03:00:00Z";const end="2026-09-26T04:00:00Z";
function envelope(revision=1):DesktopActiveTimeEnvelope {
  return {schemaVersion:"1.0",historyContextId:"context-a",status:"available",reasonCode:"activity_available",revision,timer:null,observations:[],includedCount30Days:0,includedSeconds30Days:0,hasOtherEnvironmentRecords:false,generatedAt:new Date(now).toISOString()};
}
function timer(state:ActiveTimer["state"]="running"):ActiveTimer {
  return {timerId:"private-timer-id",revision:1,state,startedAt:start,endedAt:state==="review"?end:null,suggestedSeconds:600,updatedAt:end,interrupted:false};
}
const request:DesktopActiveTimeUpdate={historyContextId:"context-a",action:{kind:"start",timerId:"same-request-id"}};
function deferred<T>() {let resolve!:(value:T)=>void;const promise=new Promise<T>((yes)=>{resolve=yes;});return {promise,resolve};}
function render(value:DesktopActiveTimeEnvelope|null,props:Partial<Parameters<typeof CapacityActiveTimeView>[0]>={}) {
  return renderToStaticMarkup(<CapacityActiveTimeView contextId="context-a" language="zh" value={value} now={now} onSave={async()=>true} onRetry={()=>undefined} {...props}/>);
}
describe("explicit activity records",()=>{
  it("validates minutes against a UTC interval and does not accept future/invalid dates",()=>{
    const from=localActivityInput(start),to=localActivityInput(end);
    expect(activityFromForm(from,to,"30",now)).toEqual({startedAt:new Date(start).toISOString(),endedAt:new Date(end).toISOString(),durationSeconds:1800});
    for (const value of ["","0","-1","NaN","Infinity","61"]) expect(activityFromForm(from,to,value,now)).toBeNull();
    expect(activityFromForm(from,to,"30",now-1000)).toBeNull();
    expect(activityFromForm("2026-02-30T00:00",to,"30",now)).toBeNull();
    expect(activityFromForm("2026-09-01T00:00",to,"30",now)).toBeNull();
    expect(activityFromForm(from,to,"0.02",now)).toMatchObject({durationSeconds:1});
  });
  it("rejects ambiguous manual DST folds but preserves an unchanged timer's exact UTC",()=>{
    const before=process.env.TZ;
    try {
      process.env.TZ="America/New_York";
      const later=Date.parse("2026-11-02T00:00:00Z");
      expect(activityFromForm("2026-11-01T01:20","2026-11-01T02:30","30",later)).toBeNull();
      expect(activityFromForm("2026-03-08T02:20","2026-03-08T03:30","30",Date.parse("2026-03-09T00:00:00Z"))).toBeNull();
      const original={startedAt:"2026-11-01T06:20:00Z",endedAt:"2026-11-01T07:30:00Z",durationSeconds:1800};
      expect(activityFromForm(localActivityInput(original.startedAt),localActivityInput(original.endedAt),"30",later,1800,original)?.startedAt).toBe("2026-11-01T06:20:00.000Z");
      expect(activityFromForm("2026-11-01T01:20","2026-11-01T02:30","30",later,1800,original)?.startedAt).toBe("2026-11-01T06:20:00.000Z");
    } finally {if (before===undefined) delete process.env.TZ;else process.env.TZ=before;}
  });
  it("only advances running suggestions and caps unusual wall time",()=>{
    expect(suggestedTimerSeconds(timer(),now+20_000)).toBe(620);
    expect(suggestedTimerSeconds(timer("paused"),now+20_000)).toBe(600);
    expect(suggestedTimerSeconds(timer(),now-20_000)).toBe(600);
    expect(suggestedTimerSeconds(timer(),now+200_000_000)).toBe(86400);
    expect(timerDisplay(3661)).toBe("01:01:01");
  });
  it("keeps revision ordering, clears private data on binding loss, and preserves read failure feedback",()=>{
    expect(acceptActiveTime(envelope(4),envelope(2),"context-a")?.revision).toBe(4);
    expect(acceptActiveTime(envelope(),{...envelope(),historyContextId:"b"},"context-a")).toBeNull();
    expect(acceptActiveTime(envelope(4),{...envelope(0),status:"unavailable"},"context-a")?.status).toBe("unavailable");
    const failed=acceptActiveTime(envelope(4),{...envelope(0),status:"failed",reasonCode:"activity_read_failed"},"context-a");
    expect(failed?.revision).toBe(4);expect(failed?.reasonCode).toBe("activity_read_failed");
  });
  it("never shows timer suggestions as confirmed use or a forecast",()=>{
    const html=render({...envelope(),timer:timer()});
    expect(html).toContain("00:10:00");expect(html).toContain("计时参考 · 未计入记录");expect(html).toContain("最近 30 天已确认 · 0 条");expect(html).toContain("不监听键盘、窗口或对话");
    expect(html).not.toContain("private-timer-id");expect(html).not.toContain("activity_available");
    expect(render(envelope(),{language:"en"})).toContain("not an available-hours forecast");
  });
  it("shows interrupted review explicitly and keeps its confirmation form out of the menu",()=>{
    const html=render({...envelope(),timer:{...timer("review"),interrupted:true}},{compact:true,onOpen:()=>undefined});
    expect(html).toContain("仅保留上次记录点");expect(html).toContain("核对并保存");expect(html).not.toContain("<form");expect(html).not.toContain("继续计时");
  });
  it("renders confirmed sources and exclusion without implementation identifiers",()=>{
    const value=envelope();value.observations=[{observationId:"private-record",source:"user_reported",quality:"user_confirmed",observation:{startedAt:start,endedAt:end,durationSeconds:1800},included:false,revision:2,createdAt:end}];
    value.hasOtherEnvironmentRecords=true;
    const html=render(value);expect(html).toContain("手动确认");expect(html).toContain("已排除");expect(html).toContain("重新纳入");expect(html).toContain("未自动合并");
    for (const code of ["private-record","user_reported","user_confirmed"]) expect(html).not.toContain(code);
    expect(activityMessage("unknown_internal_code","zh")).not.toContain("unknown_internal_code");
  });
  it("does not show a previous account's timer even with a mismatched response",()=>{
    const html=render({...envelope(),historyContextId:"b",timer:timer()});expect(html).not.toContain("00:10:00");expect(html).not.toContain("开始计时");
    expect(render({...envelope(),timer:timer()},{failed:true})).toContain("disabled");
  });
  it("coalesces reads while honoring a trailing cross-view change",async()=>{
    const pending=deferred<DesktopActiveTimeEnvelope>();let changed!:()=>void;
    const read=vi.fn().mockImplementationOnce(()=>pending.promise).mockResolvedValue(envelope(3));
    const s=createActiveTimeStore("context-a",{read,update:async()=>envelope(),subscribe:(cb)=>{changed=cb;return ()=>undefined;}});
    const dispose=s.subscribe(()=>undefined);const task=s.reload();changed();pending.resolve(envelope());await task;
    expect(read).toHaveBeenCalledTimes(2);expect(s.getSnapshot().value?.revision).toBe(3);dispose();
  });
  it("ignores a pre-save read and rejects duplicate or cross-context submissions",async()=>{
    const pending=deferred<DesktopActiveTimeEnvelope>(),save=deferred<DesktopActiveTimeEnvelope>();
    const api={read:vi.fn().mockImplementationOnce(()=>pending.promise).mockResolvedValue(envelope(4)),update:vi.fn(()=>save.promise)};
    const s=createActiveTimeStore("context-a",api);const reading=s.reload();
    expect(await s.save({...request,historyContextId:"b"})).toBe(false);
    const saving=s.save(request);expect(await s.save(request)).toBe(false);
    save.resolve({...envelope(4),status:"updated"});expect(await saving).toBe(true);pending.resolve(envelope());await reading;
    expect(s.getSnapshot().value?.revision).toBe(4);expect(api.update).toHaveBeenCalledTimes(1);
  });
  it("retains save errors and the same request ID so lost acknowledgments are retryable",async()=>{
    const api={read:vi.fn(async()=>envelope()),update:vi.fn().mockRejectedValueOnce(new Error("private detail")).mockResolvedValue({...envelope(2),status:"updated"})};
    const s=createActiveTimeStore("context-a",api);await s.reload();
    expect(await s.save(request)).toBe(false);expect(s.getSnapshot().value?.reasonCode).toBe("activity_save_failed");
    expect(await s.save(request)).toBe(true);expect(api.update.mock.calls[1][0]).toEqual(request);
  });
});
