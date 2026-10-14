import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { quantitativeFixture } from "../../preview/quantitativeFixture";
import { ResetBriefing } from "./ResetBriefing";
import { forecastPresentation } from "./resetBriefingModel";
import { activeForecastAnnouncement, combinedStatements, communityEffect, evaluationStale, forecastFor, forecastStale, seriesPaths, sourceRows } from "./quantitativeModel";
import { ResetNoticeChip } from "../../components/CapacityPopover/ResetNoticeChip";
import { TiboBriefing } from "./TiboBriefing";
import type { PublicResetTimeline } from "./types";
const now = Date.parse("2026-10-04T02:10:00Z");
const timeline = (): PublicResetTimeline => ({sources:[],entries:[],revisionCount:0,evidenceFamilyCount:0,quantitative:quantitativeFixture(now)});
describe("independent quantitative integration", () => {
  it("keeps the separate commitment estimate visible beside original window values", () => {
    const t=timeline(), f=forecastFor(t.quantitative)!;
    f.sources=[{id:"codex_reset",probabilities:Array(72).fill(.83),source_points:[[24,.45],[48,.70]],
      origin:now/1000,source_coverage_hours:48,weight:1,
      comment:{probability:.83,url:"https://x.com/i/status/1234",at:now/1000-3600,included:true,state:"included",deadline:now/1000+3600,timing_sources:["nextreset"],adjustment_24h:.38,adjustment_48h:.13}}];
    const row=sourceRows(t.quantitative!).find(s=>s.source==="codex_reset" && s.target===f.target)!;
    expect(row.probabilities_at_publication[24]).toBe(.45);
    expect(row.comment?.probability).toBe(.83);
    const zh=renderToStaticMarkup(<ResetBriefing timeline={t} now={now} language="zh" />);
    expect(zh).toContain("承诺判断 83.0% · 已计入");
    const en=renderToStaticMarkup(<ResetBriefing timeline={t} now={now} language="en" />);
    expect(en).toContain("Commitment estimate 83.0% · Included");
  });
  it("keeps an upstream announcement readable without changing the independent probability", () => {
    const t=timeline(), f=forecastFor(t.quantitative)!;
    f.curve=f.curve.map(p=>({...p,probability:.78}));
    f.response={at:now/1000-3600,url:"https://x.com/i/status/1234",reviewed:["monitor","nextreset"],changed:[],kind:"announcement",deadline:now/1000+3600};
    f.statements=[
      {id:"1234",at:now/1000-3600,url:"https://x.com/thsottiaux/status/1234",original:"Global reset by EOD.",translation_zh:"今日结束前全球重置。",stage:"scheduled",event_type:"usage_reset",context:null},
      {id:"5678",at:now/1000-60,url:"https://x.com/thsottiaux/status/5678",original:"A newer ordinary reply.",translation_zh:null,stage:null,event_type:null,context:null},
    ];
    expect(activeForecastAnnouncement(t,now)?.id).toBe("1234");
    const html=renderToStaticMarkup(<TiboBriefing timeline={t} notice={null} language="zh" now={now} />);
    expect(html.indexOf("Global reset by EOD.")).toBeLessThan(html.indexOf("A newer ordinary reply."));
    expect(html).toContain("今日结束前全球重置。");
    expect(forecastPresentation(undefined,now,false,t).probability24h).toBe(78);
    expect(activeForecastAnnouncement(t,now+3600_000)).toBeUndefined();
    f.statements![0].stage="completed";
    expect(activeForecastAnnouncement(t,now)).toBeUndefined();
  });
  it("recognizes a completion immediately even while forecasts and scope verification lag", () => {
    const t=timeline(), f=forecastFor(t.quantitative)!;
    const at=(now-10_000)/1000;
    f.last_event_at=at-86400;
    f.statements=[{id:"1234",at,url:"https://x.com/thsottiaux/status/1234",original:"The reset has been processed. Enjoy!",
      translation_zh:"重置已经处理完成。尽情享受！",stage:"completed",event_type:"usage_reset",context:null}];
    const presentation=forecastPresentation(undefined,now,false,t);
    expect(presentation.awaitingResetForecast).toBe(true);
    expect(presentation.probability24h).toBeNull();
    const html=renderToStaticMarkup(<ResetBriefing timeline={t} now={now} language="zh" />);
    expect(html).toContain("Horizon 综合判断");
    expect(html).not.toContain("已发布重置完成公告");
    expect(html).toContain("The reset has been processed. Enjoy!");
    expect(html).toContain("重置已经处理完成。尽情享受！");
    f.last_event_at=at;
    expect(forecastPresentation(undefined,now,false,t).awaitingResetForecast).toBe(false);
  });
  it("keeps completion and reply context when the second collector has only a partial post", () => {
    const t=timeline(), f=forecastFor(t.quantitative)!;
    f.statements=[{id:"1234",at:now/1000,url:"https://x.com/thsottiaux/status/1234",original:"Why not?",translation_zh:"为什么不呢？",
      stage:null,event_type:null,context:null,is_reply:true,relevance:"reset_related",parent:{id:"1233",author:"example",url:"https://x.com/example/status/1233",text:"Will there be a reset within 24h?"}}];
    const p=combinedStatements(t)[0];
    t.insights={forecast:{value:null,attemptedAt:null,issue:null},posts:{checkedAt:f.issued_at,fetchedAt:f.issued_at,posts:[{...p,parent:null,translatedText:null}]}};
    const post=combinedStatements(t)[0];
    expect(post.parent?.text).toBe("Will there be a reset within 24h?");
    expect(post.translatedText).toBe("为什么不呢？");
    const html=renderToStaticMarkup(<ResetBriefing timeline={t} now={now} language="en" />);
    expect(html).toContain("Why not?");expect(html).toContain("Will there be a reset within 24h?");
  });
  it("shows the additive anchor and revisions rather than three competing probability budgets", () => {
    const t=timeline(), f=forecastFor(t.quantitative)!;
    f.combination_method="baseline_plus_revisions";
    f.curve=f.curve.map(p=>({...p,baseline:.2,radar_adjustment:.3,community_adjustment:0,coherence_adjustment:0,probability:.5}));
    const en=renderToStaticMarkup(<ResetBriefing timeline={t} now={now} language="en" />);
    for(const text of ["Probability breakdown", "Historical anchor", "Source revisions", "20.0%", "+30.0 pts", "Recent ballots"]) expect(en).toContain(text);
    expect(en).not.toContain('aria-label="Applied weights"');
    const zh=renderToStaticMarkup(<ResetBriefing timeline={t} now={now} language="zh" />);
    expect(zh).toContain("概率组成");
    f.curve=f.curve.map(p=>({...p,comment_adjustment:.1}));
    const split=renderToStaticMarkup(<ResetBriefing timeline={t} now={now} language="zh" />);
    expect(split).toContain("承诺判断补充");
    expect(split).toContain("+20.0 点");
    expect(split).toContain("+10.0 点");
  });
  it("shows actual family and ballot weights, sample quality and unavailable indicators", () => {
    const t=timeline(), f=forecastFor(t.quantitative)!;
    f.family_weights={baseline:.15,radars:.75,community:.1};
    f.community_inputs=[{id:"date_poll",weight:.1,votes:70,effective_votes:8.2,coverage_hours:36,probability_24h:.3,probability_48h:.5,latest_vote_at:now/1000-3600,freshness_weight:.7}];
    f.community_coverage={included:1,unavailable:1,supplementary_included:0,supplementary_checked:1,supplementary_weight:0};
    const en=renderToStaticMarkup(<ResetBriefing timeline={t} now={now} language="en" />);
    for (const text of ["Complete radar forecasts", "75.0%", "70 votes", "Effective sample 8.2", "Applied weight", "baseline after deadline", "1 eligible", "0/1 admitted"]) expect(en).toContain(text);
    expect(en).not.toContain("market");
    const zh=renderToStaticMarkup(<ResetBriefing timeline={t} now={now} language="zh" />);
    expect(zh).toContain("雷达完整预测");expect(zh).toContain("有效样本 8.2");
    expect(zh).toContain("最近检测到新增票：");
    expect(en).toContain("Latest new ballots observed:");
  });
  it("shows known-event errors separately without deriving calibration from positives alone", () => {
    const t=timeline(), card=t.quantitative!.evaluation!.scorecards.find(r=>r.track==="model")!;
    card.observed_event_scores={...card.scores,n:2,unique_events:1,brier:.123,log_loss:.4};
    const html=renderToStaticMarkup(<ResetBriefing timeline={t} now={now} language="en" />);
    expect(html).toContain("Additional event checks");
    expect(html).toContain("Brier 0.123");
    expect(html).not.toContain("Probability calibration");
  });
  it("replaces a source's obsolete raw series in the comparison while retaining its history elsewhere", () => {
    const v=quantitativeFixture(now), old=v.evaluation!.latest.find(r=>r.source==="codex_reset" && r.track==="native")!;
    const updated={...old,version:"source:published-v2",origin:new Date(now).toISOString(),probabilities_at_publication:{24:.45,48:.7}};
    v.evaluation!.latest.push(updated);
    const rows=sourceRows(v).filter(r=>r.source==="codex_reset" && r.target===old.target);
    expect(rows).toHaveLength(1);expect(rows[0].probabilities_at_publication[24]).toBe(.45);
  });
  it("keeps full bilingual posts and progress inside the focusable Tibo panel", () => {
    const t=timeline(), f=forecastFor(t.quantitative)!;
    const original="Synthetic detailed reset statement. ".repeat(40)+"Original ending retained.";
    const translation="合成的完整重置发言。".repeat(40)+"译文结尾保留。";
    f.statements=[{id:"long-post",at:(now-1000)/1000,url:"https://x.com/i/status/123",original,
      translation_zh:translation,stage:"completed",event_type:"usage_reset",context:null}];
    const html=renderToStaticMarkup(<TiboBriefing className="aligned-panel" timeline={t} notice={null} language="zh" now={now}>
      <section aria-label="Reset timing and progress">Progress remains reachable</section>
    </TiboBriefing>);
    expect(html).toContain('aligned-panel');
    expect(html).toContain('tabindex="0" aria-label="Tibo 发言与互动"');
    expect(html).toContain(original);expect(html).toContain(translation);
    expect(html).toContain('Progress remains reachable</section></article>');
  });
  it("keeps activities in the ordinary post list as newer replies and completions arrive", () => {
    const t=timeline(), f=forecastFor(t.quantitative)!;
    f.statements=[
      {id:"activity",at:now/1000-86400,url:"https://x.com/thsottiaux/status/123",original:"For the next 28 days, each day we either ship improvements or ship a full reset.",translation_zh:"接下来 28 天，每天推出改进或进行完整重置。",stage:null,event_type:null,context:null},
      {id:"completion",at:now/1000-3600,url:"https://x.com/thsottiaux/status/124",original:"The reset has been processed.",translation_zh:"重置已经完成。",stage:"completed",event_type:"usage_reset",context:null},
      {id:"reply",at:now/1000-60,url:"https://x.com/thsottiaux/status/125",original:"We are checking the affected accounts.",translation_zh:"我们正在检查受影响的账号。",stage:null,event_type:null,context:null,is_reply:true,relevance:"reset_related",parent:{id:"question",author:"example",url:"https://x.com/example/status/126",text:"My reset has not arrived yet."}},
    ];
    for (const at of [now,now+30*86400_000]) {
      const html=renderToStaticMarkup(<TiboBriefing timeline={t} notice={null} language="zh" now={at} />);
      expect(html).toContain("My reset has not arrived yet.");
      expect(html).toContain("We are checking the affected accounts.");
      expect(html).toContain("我们正在检查受影响的账号。");
      const list=html.slice(html.indexOf("其他动态与回复"));
      expect(list.indexOf("重置已经完成。")).toBeGreaterThan(0);
      expect(list.indexOf("接下来 28 天")).toBeGreaterThan(list.indexOf("重置已经完成。"));
      expect(html).not.toContain("持续关注的重置承诺");
      expect(html).not.toContain("<details");
    }
    const en=renderToStaticMarkup(<TiboBriefing timeline={t} notice={null} language="en" now={now} />);
    expect(en).toContain("For the next 28 days");
    expect(en).not.toContain("接下来 28 天");
  });
  it("renders matched probability changes and separates weights from input movement", () => {
    const t=timeline(), f=forecastFor(t.quantitative)!;
    f.changes={since:new Date(now-3600_000).toISOString(),until:f.issued_at,new_statement_ids:[],windows:[24,48].map(hours=>({hours,before:.4,after:.5,delta:.1,coherence_effect:0,drivers:[{id:"community",before:.6,after:.8,weight_before:.5,weight_after:.5,raw_before:null,raw_after:null,input_effect:.1,weight_effect:0,effect:.1}]}))};
    const html=renderToStaticMarkup(<ResetBriefing timeline={t} now={now} language="en" />);
    expect(html).toContain("Why the outlook changed");expect(html).toContain("+10.0");expect(html).toContain("Weight effect");
    expect(html).not.toContain("market");
  });
  it("uses the independent probability in the main overview and menu, without needing the old radar cache", () => {
    const t=timeline();
    expect(forecastPresentation(undefined,now,false,t).probability24h).toBeCloseTo(62);
    const html=renderToStaticMarkup(<ResetBriefing timeline={t} now={now} language="en" />);
    expect(html).toContain("62.0%");expect(html).toContain("72.0%");
    expect(html).toContain("Reset Observatory");expect(html).toContain("Lunar");
    expect(html).toContain("Tibo");expect(html).toContain("Collecting the first forward results");
    expect(html).not.toContain("horizon-pool-v3");
    expect(html).toContain('data-series="monitor-automatic_reset-24"');
    expect(html).toContain('data-series="monitor-automatic_reset-48"');
    expect(renderToStaticMarkup(<ResetNoticeChip timeline={t} now={now} language="en" onOpen={()=>{}} />)).toContain("Horizon 24h 62%");
  });
  it("retains saved values with a stale label and never silently falls back to the old estimate", () => {
    const t=timeline();const v=t.quantitative!;
    expect(forecastStale(v,forecastFor(v),now+900_000)).toBe(true);
    expect(evaluationStale(v,now+180_000)).toBe(true);
    const html=renderToStaticMarkup(<ResetBriefing timeline={t} now={now+900_000} language="en" />);
    expect(html).toContain("Saved forecast · update due");expect(html).toContain("saved report");
    v.forecast=null;v.forecastIssue="missing";
    expect(forecastPresentation(undefined,now,false,t).probability24h).toBeNull();
    const missing=renderToStaticMarkup(<ResetBriefing timeline={t} now={now} language="en" />);
    expect(missing).toContain("Awaiting forecast");expect(missing).not.toContain("0.0%");
  });
  it("only shows a community effect when the ablation belongs to the exact forecast issue", () => {
    const v=quantitativeFixture(now),f=forecastFor(v)!;
    expect(communityEffect(v.evaluation,f,24)).toBeCloseTo(.12);
    f.issued_at=new Date(now).toISOString();
    expect(communityEffect(v.evaluation,f,24)).toBeNull();
  });
  it("preserves missing history and never bridges unsupported windows or long collection gaps", () => {
    const p=[{at:new Date(0).toISOString(),p:.2},{at:new Date(3600_000).toISOString(),p:null},{at:new Date(7200_000).toISOString(),p:.4},{at:new Date(18000_000).toISOString(),p:.6}];
    const paths=seriesPaths(p,0,20000_000,n=>n/3600_000,n=>n*100);
    expect(paths).toEqual(["M0.00,20.00","M2.00,40.00","M5.00,60.00"]);
  });
  it("shows proper scores and evidence states once mature results exist", () => {
    const t=timeline();t.quantitative=quantitativeFixture(now,true);
    const html=renderToStaticMarkup(<ResetBriefing timeline={t} now={now} language="en" />);
    for(const text of ["0.184","0.526","Probability calibration","Timing or scope disputed","Targeted compensation","Awaiting at least 8 weekly blocks"]) expect(html).toContain(text);
    expect(html).not.toContain("Collecting the first forward results");
  });
  it("distinguishes matured windows lacking evidence from windows still waiting to mature", () => {
    const t=timeline();t.quantitative!.evaluation!.summary.settlements.unresolved=380;
    const html=renderToStaticMarkup(<ResetBriefing timeline={t} now={now} language="en" />);
    expect(html).toContain("Some windows have matured; evidence is still insufficient to score");
    expect(html).not.toContain("Collecting the first forward results");
    expect(html).not.toContain("Probability calibration");
  });
  it("explains rolling-only settlement without blaming missing outcome evidence", () => {
    const t=timeline(), report=t.quantitative!.evaluation!;
    report.summary.settlements={settled:220,unresolved:3398,pending:4198};
    report.summary.scoring_by_horizon=[{hours:6,settled_windows:10,scored_windows:0,scored_trials:0,eligible_rolling_trials:181,missing_forecast_trials:39}];
    const html=renderToStaticMarkup(<ResetBriefing timeline={t} now={now} language="en" />);
    expect(html).toContain("Outcomes settled; awaiting non-overlapping scoring windows");
    expect(html).toContain("181");
    expect(html).not.toContain("Some windows have matured; evidence is still insufficient to score");
  });
  it("distinguishes source-level score counts from target-window counts", () => {
    const t=timeline();t.quantitative=quantitativeFixture(now,true);
    const report=t.quantitative.evaluation!;
    report.summary.scored_nonoverlapping_trials=37;
    report.summary.scoring_by_horizon=[{hours:6,settled_windows:12,scored_windows:2,scored_trials:37,eligible_rolling_trials:218,missing_forecast_trials:46}];
    const en=renderToStaticMarkup(<ResetBriefing timeline={t} now={now} language="en" />);
    expect(en).toContain("37 scores across 2 target-time windows");
    expect(en).toContain("Different sources and variants share the same windows");
    const zh=renderToStaticMarkup(<ResetBriefing timeline={t} now={now} language="zh" />);
    expect(zh).toContain("37 条评分，覆盖 2 个目标时间窗");
  });
  it("explains a settled outcome with no forecast recorded at the origin", () => {
    const t=timeline(), report=t.quantitative!.evaluation!;
    report.summary.settlements={settled:2};
    report.summary.scoring_by_horizon=[{hours:6,settled_windows:1,scored_windows:0,scored_trials:0,eligible_rolling_trials:0,missing_forecast_trials:2}];
    const html=renderToStaticMarkup(<ResetBriefing timeline={t} now={now} language="en" />);
    expect(html).toContain("Outcomes settled; no eligible forecast at those origins");
    expect(html).not.toContain("Collecting the first forward results");
  });
  it("shows managed startup failures and retry time without discarding the saved forecast", () => {
    const t=timeline();t.quantitative!.runtime={managed:true,state:"retrying",checkedAt:new Date(now).toISOString(),retryAt:new Date(now+60_000).toISOString(),issue:"environment_unavailable"};
    const html=renderToStaticMarkup(<ResetBriefing timeline={t} now={now} language="en" />);
    expect(html).toContain("Module startup needs recovery");expect(html).toContain("Next retry");
    expect(html).toContain("62.0%");expect(html).not.toContain("environment_unavailable");
  });
  it("merges archived statements with live replies and keeps translations out of English posts", () => {
    const t=timeline(),f=forecastFor(t.quantitative)!;
    f.statements=[{id:"post",at:(now-1000)/1000,url:"https://x.com/i/status/123",original:"A full public reply",translation_zh:"完整中文译文",stage:null,event_type:null,context:{text:"中文上文",url:"https://x.com/example/status/122"}}];
    expect(combinedStatements(t)).toHaveLength(1);
    const en=renderToStaticMarkup(<ResetBriefing timeline={t} now={now} language="en" />);
    expect(en).toContain("A full public reply");expect(en).not.toContain("完整中文译文");expect(en).not.toContain("中文上文");
    const zh=renderToStaticMarkup(<ResetBriefing timeline={t} now={now} language="zh" />);
    expect(zh).toContain("完整中文译文");expect(zh).toContain("中文上文");
    t.insights={forecast:{value:null,attemptedAt:null,issue:null},posts:{checkedAt:f.issued_at,fetchedAt:f.issued_at,posts:[{...combinedStatements(t)[0],text:"Live version",parent:{id:"parent",author:"example",url:"https://x.com/example/status/122",text:"Full parent",checkedAt:f.issued_at}}]}};
    expect(combinedStatements(t)).toHaveLength(1);expect(combinedStatements(t)[0].text).toBe("Live version");
  });
});
