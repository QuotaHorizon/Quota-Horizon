import {renderToStaticMarkup} from "react-dom/server";
import {describe,expect,it} from "vitest";
import type {PaceEvidenceSummary} from "../../../../capacity-preview/src/status";
import {PaceEvidenceView} from "./PaceEvidenceView";
import {PaceTrialHistoryView,trialReason} from "./PaceTrialHistory";
function evidence():PaceEvidenceSummary {
  const counts={total:8,pending:3,within:1,below:1,above:1,unscorable:2};
  return {generatedAt:"2026-09-26T07:00:00Z",oldestIssuedAt:"2026-09-25T06:00:00Z",newestIssuedAt:"2026-09-26T06:00:00Z",limited:false,counts,groups:[{algorithmVersion:"recent-block-pace-experimental-v1",verifierVersion:"pace-balance-outcome-v1",windowMinutes:10080,compatibilityUnverified:true,counts,trainingMaximumGapSeconds:[300,900],trainingRateRatio:[1,2.5]}],unscorableReasons:[{reasonCode:"target_samples_missing",count:2}]};
}
function render(value=evidence(),language:"en"|"zh"="zh") {return renderToStaticMarkup(<PaceEvidenceView value={value} language={language} reasonText={reason=>trialReason(reason,language)}/>);}
describe("retained prospective evidence",()=>{
  it("exposes exact denominators and never counts missing or pending as hits",()=>{
    const html=render();expect(html).toContain("33.3% (1/3)");expect(html).toContain("40% (2/5)");expect(html).toContain("偏乐观");expect(html).toContain("偏保守");expect(html).toContain("不限于本页");expect(html).toContain("已保存的实验推算");expect(html).toContain("待核验记录单列");
  });
  it("shows unavailable ratios rather than invented zero accuracy",()=>{
    const value=evidence();value.counts={total:3,pending:3,within:0,below:0,above:0,unscorable:0};value.groups=[];value.unscorableReasons=[];
    const html=render(value);expect(html).toContain("可比较：—");expect(html).toContain("已终结：—");expect(html).not.toContain("0%");
  });
  it("does not conflate inconclusive comparisons with missing captures",()=>{
    const value=evidence();value.unscorableReasons=[{reasonCode:"target_samples_missing",count:1},{reasonCode:"boundary_uncertain",count:1}];
    const html=render(value);expect(html).toContain("无法核验 / 已终结：40% (2/5)");expect(html).toContain("缺读数或断档 / 已终结：20% (1/5)");
  });
  it("explains limits, sampled conditions and versions in readable language",()=>{
    const value=evidence();value.limited=true;value.groups.push({...value.groups[0],windowMinutes:300,algorithmVersion:"unknown_model_v9",verifierVersion:"unknown_verifier",compatibilityUnverified:false,trainingMaximumGapSeconds:null,trainingRateRatio:null});
    const html=render(value);expect(html).toContain("汇总最近 8 条");expect(html).toContain("5 小时额度");expect(html).toContain("5–15 分钟");expect(html).toContain("1–2.5");expect(html).toContain("其他实验版本");expect(html).toContain("来源兼容性未验证");expect(html).toContain("来源兼容性已测");
    const text=html.replace(/<[^>]+>/g,"");for(const code of ["unknown_model_v9","unknown_verifier","target_samples_missing","recent-block-pace-experimental-v1"])expect(text).not.toContain(code);
    expect(render(value,"en")).toContain("Earlier records are available");
  });
  it("keeps scope-wide summary on an empty later page and withdraws it on context loss",()=>{
    const value={historyContextId:"a",status:"available" as const,reasonCode:"trials_available",offset:20,hasMore:false,recordingFailed:false,trials:[],summary:evidence()};
    const props={value,context:"a",offset:20,failed:false,loading:false,language:"zh" as const,onReload:()=>undefined,onPage:()=>undefined};
    const html=renderToStaticMarkup(<PaceTrialHistoryView {...props}/>);expect(html).toContain("8 条记录");expect(html).toContain("请返回上一页");expect(html).not.toContain("还没有前瞻记录");
    expect(renderToStaticMarkup(<PaceTrialHistoryView {...props} failed/>)).toContain("暂保留本页上次记录");
    expect(renderToStaticMarkup(<PaceTrialHistoryView {...props} context="b"/>)).not.toContain("8 条记录");
    expect(renderToStaticMarkup(<PaceTrialHistoryView {...props} value={{...value,status:"unavailable"}}/>)).not.toContain("8 条记录");
  });
});
