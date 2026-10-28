import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { quantitativeFixture } from "../../preview/quantitativeFixture";
import { ResetBriefing } from "./ResetBriefing";
import { forecastPresentation } from "./resetBriefingModel";
import { activeForecastAnnouncement, combinedStatements, communityEffect, controllingPoint, evaluationStale, forecastFor, forecastStale, seriesPaths, sourceRows } from "./quantitativeModel";
import { ResetNoticeChip } from "../../components/CapacityPopover/ResetNoticeChip";
import { TiboBriefing } from "./TiboBriefing";
import { QuantitativeTrend } from "./QuantitativeTrend";
import { QuantitativeEvaluation } from "./QuantitativeEvaluation";
import type { PublicResetTimeline } from "./types";
const now = Date.parse("2026-10-04T02:10:00Z");
const timeline = (): PublicResetTimeline => ({sources:[],entries:[],revisionCount:0,evidenceFamilyCount:0,quantitative:quantitativeFixture(now)});
describe("independent quantitative integration", () => {
  it.each(["zh", "en"] as const)("propagates completion revisions without changing the forecast or losing the statement in %s", language => {
    const t = timeline(), f = forecastFor(t.quantitative)!;
    const postId = "2115297093879472332", published = now / 1000 - 60;
    f.cycle = null; f.last_event_at = published - 3600;
    f.statements = [{ id: postId, at: published, url: `https://x.com/thsottiaux/status/${postId}`,
      original: "https://t.co/Fgy0dBAHsJ has landed", translation_zh: null, stage: "completed", event_type: "usage_reset", context: null }];
    const event = { id: "completion-revision", status: "confirmed", reason: "public_completion_notice", kind: "automatic",
      from: published, to: published, announced_at: published, known_at: now / 1000 - 10,
      references: [`https://x.com/i/status/${postId}`], primary_references: 1, completion_reports: 1 };
    t.quantitative!.evaluation!.outcomes = [event];
    const curve = JSON.stringify(f.curve);
    expect(forecastPresentation(undefined, now, false, t).awaitingResetForecast).toBe(true);
    event.status = "pending"; event.reason = "completion_withdrawn"; event.completion_reports = 0;
    expect(forecastPresentation(undefined, now - 20_000, false, t).awaitingResetForecast).toBe(true);
    const current = forecastPresentation(undefined, now, false, t);
    expect(current.awaitingResetForecast).toBe(false);
    expect(current.probability24h).toBe(f.curve.find(p => p.hours === 24)!.probability * 100);
    const html = renderToStaticMarkup(<ResetBriefing timeline={t} now={now} language={language} />);
    expect(html).toContain("has landed");
    expect(html).toContain(language === "zh" ? "完成状态待核实" : "Completion status under review");
    expect(html).not.toContain(language === "zh" ? "等待重置后预测" : "Awaiting a post-reset forecast");
    event.references = ["https://x.com/i/status/999999"];
    expect(forecastPresentation(undefined, now, false, t).awaitingResetForecast).toBe(true);
    event.references = [`https://x.com/i/status/${postId}`];
    event.status = "confirmed"; event.reason = "public_completion_notice"; event.completion_reports = 1;
    expect(forecastPresentation(undefined, now, false, t).awaitingResetForecast).toBe(true);
    expect(JSON.stringify(f.curve)).toBe(curve);
  });
  it("attributes inherited automatic news before the broader community supplement", () => {
    const t = timeline(), broad = forecastFor(t.quantitative)!, auto = forecastFor(t.quantitative, "automatic_reset")!;
    broad.combination_method = "baseline_plus_revisions";
    auto.curve = auto.curve.map(p => ({ ...p, baseline: .2, radar_adjustment: .6, community_adjustment: 0,
      coherence_adjustment: 0, probability: .8 }));
    broad.curve = broad.curve.map(p => ({ ...p, baseline: .17, radar_adjustment: 0, community_adjustment: .02,
      coherence_adjustment: .63, probability: .82 }));
    const point = controllingPoint(broad, auto, 24)!;
    expect(point.baseline).toBe(.2);
    expect(point.radar_adjustment).toBe(.6);
    expect(point.community_adjustment).toBeCloseTo(.02);
    expect(point.coherence_adjustment).toBe(0);
    expect(point.baseline + point.radar_adjustment! + point.community_adjustment! + point.coherence_adjustment!).toBeCloseTo(.82);
    expect(renderToStaticMarkup(<ResetBriefing timeline={t} now={now} language="zh" />)).toContain("+60.0 点");
  });
  it.each(["zh", "en"] as const)("preserves outcome provider failure states and legacy reports in %s", language => {
    const view = quantitativeFixture(now, true);
    const cases = [
      ["healthy", true, "可用", "Available"],
      ["positive_evidence", false, "完成证据", "Completion evidence"],
      ["collector_overdue", true, "采集已逾期", "Collection overdue"],
      ["request_or_decode_failure", false, "读取失败", "Read failed"],
      ["stale_source_content", false, "来源内容已过期", "Source content stale"],
      ["incomplete_source_evidence", false, "来源证据不完整", "Source evidence incomplete"],
      [undefined, true, "可用", "Available"],
      [undefined, false, "待更新", "Update due"],
    ] as const;
    for (const [state, healthy, zh, en] of cases) {
      view.evaluation!.outcome_providers = [{provider:"quota_events", last_attempt:new Date(now).toISOString(), generated_at:new Date(now).toISOString(), healthy, state}];
      const html = renderToStaticMarkup(<QuantitativeEvaluation view={view} language={language} now={now} />);
      expect(html).toContain(`QuotaResets · ${language === "zh" ? zh : en}`);
      expect(html).not.toContain("collector_overdue");
      expect(html).not.toContain("request_or_decode_failure");
    }
  });
  it.each(["zh", "en"] as const)("distinguishes report read failures from first-run waiting in %s", language => {
    const view=quantitativeFixture(now);
    view.evaluation=null;view.evaluationIssue=null;
    const render=()=>[
      renderToStaticMarkup(<QuantitativeTrend view={view} language={language} now={now} />),
      renderToStaticMarkup(<QuantitativeEvaluation view={view} language={language} now={now} />),
    ];
    const waiting=language === "zh" ? ["等待评估器的首个整点观测", "等待独立评估器输出"] : ["Awaiting the evaluator’s first hourly observation", "Waiting for the independent evaluator"];
    const error=language === "zh" ? "评估报告读取失败，后台会自动重试。" : "The evaluation report could not be read. Retrying automatically in the background.";
    render().forEach((html,i)=>{ expect(html).toContain(waiting[i]);expect(html).not.toContain(error); });
    view.evaluationIssue="invalid";
    render().forEach((html,i)=>{ expect(html).toContain(error);expect(html).not.toContain(waiting[i]); });
  });
  it.each(["zh", "en"] as const)("retains saved curves and scores after a report read failure in %s", language => {
    const view=quantitativeFixture(now,true);
    const before=renderToStaticMarkup(<QuantitativeTrend view={view} language={language} now={now} />);
    expect(before).toMatch(/<path d="M[^\"]+L/);
    view.evaluationIssue="invalid";
    expect(renderToStaticMarkup(<QuantitativeTrend view={view} language={language} now={now} />)).toBe(before);
    const report=renderToStaticMarkup(<QuantitativeEvaluation view={view} language={language} now={now} />);
    expect(report).toContain("0.184");
    expect(report).toContain(language === "zh" ? "评估器最近未更新，以下为保存的报告。" : "The evaluator has not updated recently. Showing the saved report.");
    expect(report).not.toContain(language === "zh" ? "评估报告读取失败" : "The evaluation report could not be read");
  });
  it("keeps the separate commitment estimate visible beside original window values", () => {
    const t=timeline(), f=forecastFor(t.quantitative)!;
    f.sources=[{id:"codex_reset",probabilities:Array(72).fill(.83),source_points:[[24,.45],[48,.70]],
      origin:now/1000,source_coverage_hours:48,weight:1,
      comment:{probability:.83,url:"https://x.com/i/status/1234",at:now/1000-3600,included:true,state:"included",deadline:now/1000+3600,timing_sources:["nextreset"],timing_evidence:[{source:"nextreset",deadline:now/1000+3600,relationship:"direct_reply",estimate_url:"https://x.com/i/status/1234",timing_url:"https://x.com/i/status/5678",links:[{source:"tibo_posts",child_url:"https://x.com/i/status/5678",parent_url:"https://x.com/i/status/1234",at:now/1000-1800}]}],adjustment_24h:.38,adjustment_48h:.13}}];
    const row=sourceRows(t.quantitative!).find(s=>s.source==="codex_reset" && s.target===f.target)!;
    expect(row.probabilities_at_publication[24]).toBe(.45);
    expect(row.comment?.probability).toBe(.83);
    const zh=renderToStaticMarkup(<ResetBriefing timeline={t} now={now} language="zh" />);
    expect(zh).toContain("承诺判断 83.0% · 已计入");
    const en=renderToStaticMarkup(<ResetBriefing timeline={t} now={now} language="en" />);
    expect(en).toContain("Commitment estimate 83.0% · Included");
    expect(zh).toContain("关联回复与时间");
    expect(en).toContain("Linked reply and timing");
    expect(en).toContain('href="https://x.com/i/status/5678"');
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
  it("keeps a later announcement of the same observed reset from closing its new cycle", () => {
    const t = timeline(), f = forecastFor(t.quantitative)!;
    const occurred = now / 1000 - 3600;
    f.last_event_at = occurred;
    f.cycle = { time_basis: "observed_reset", occurred_at: occurred, confirmation_at: now / 1000 - 60,
      invalidation_at: occurred, references: ["https://x.com/i/status/1234"], identity: "event:1234" };
    f.statements = [{ id: "1234", at: now / 1000 - 60, url: "https://x.com/thsottiaux/status/1234",
      original: "Completed earlier.", translation_zh: null, stage: "completed", event_type: "usage_reset", context: null }];
    expect(forecastPresentation(undefined, now, false, t).awaitingResetForecast).toBe(false);
    f.statements[0] = { ...f.statements[0], id: "5678", url: "https://x.com/thsottiaux/status/5678" };
    expect(forecastPresentation(undefined, now, false, t).awaitingResetForecast).toBe(true);
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
  it.each(["zh", "en"] as const)("keeps the consensus deadline estimate distinct from its 24h and 48h projection in %s", language => {
    const t = timeline(), f = forecastFor(t.quantitative)!;
    const deadline = now / 1000 + 3 * 3600;
    f.community_inputs = [{ id: "didcodex_poll", weight: .78, votes: 6960, recent_votes: 6960,
      total_votes: 30447, effective_votes: null, coverage_hours: 3, probability_24h: .98,
      probability_48h: .99, mapping_kind: "sentiment_level", mapping_version: "calendar-consensus-2",
      terminal_probability: .977, window_end: deadline }];
    const html = renderToStaticMarkup(<ResetBriefing timeline={t} now={now} language={language} />);
    expect(html).toContain(language === "zh" ? "原期限内预估 97.7%" : "Estimate by original deadline 97.7%");
    expect(html).toContain(`dateTime="${new Date(deadline * 1000).toISOString()}"`);
    expect(html).toContain(language === "zh" ? "已折减重复点击影响" : "repeated-click sensitivity applied");
    expect(html).toContain("24h 98.0%");
    expect(html).toContain("48h 99.0%");
    expect(html).not.toContain(language === "zh" ? "有效样本 —" : "Effective sample —");
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
  it("marks old source windows closed as soon as the producer detects a new reset", () => {
    const v=quantitativeFixture(now), f=forecastFor(v, "automatic_reset")!;
    f.last_event_at=now/1000-60;
    f.sources=[];
    const raw=v.evaluation!.latest.find(r=>r.source==="monitor" && r.track==="native")!;
    raw.origin=new Date(now-3600_000).toISOString();raw.state="current";
    const row=sourceRows(v).find(r=>r.source==="monitor" && r.target===f.target)!;
    expect(row.state).toBe("cycle_closed");expect(row.included).toBe(false);
    expect(row.probabilities_at_publication).toEqual(raw.probabilities_at_publication);
    raw.origin=new Date(now).toISOString();raw.state="cycle_closed";
    expect(sourceRows(v).find(r=>r.source==="monitor" && r.target===f.target)!.state).toBe("cycle_closed");
  });
  it("keeps full bilingual posts and progress inside the focusable Tibo body", () => {
    const t=timeline(), f=forecastFor(t.quantitative)!;
    const original="Synthetic detailed reset statement. ".repeat(40)+"Original ending retained.";
    const translation="合成的完整重置发言。".repeat(40)+"译文结尾保留。";
    f.statements=[{id:"long-post",at:(now-1000)/1000,url:"https://x.com/i/status/123",original,
      translation_zh:translation,stage:"completed",event_type:"usage_reset",context:null}];
    const html=renderToStaticMarkup(<TiboBriefing className="aligned-panel" bodyClassName="scrolling-body" timeline={t} notice={null} language="zh" now={now}>
      <section aria-label="Reset timing and progress">Progress remains reachable</section>
    </TiboBriefing>);
    expect(html).toContain('aligned-panel');
    expect(html).toContain('scrolling-body" tabindex="0" aria-label="发言正文与互动列表"');
    expect(html).toContain(original);expect(html).toContain(translation);
    expect(html).toContain('Progress remains reachable</section></div></article>');
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
    expect(missing).toContain("Awaiting forecast");expect(missing).not.toMatch(/<strong[^>]*>0\.0%<\/strong>/);
  });
  it("only shows a community effect when the ablation belongs to the exact forecast issue", () => {
    const v=quantitativeFixture(now),f=forecastFor(v)!;
    expect(communityEffect(v.evaluation,f,24)).toBeCloseTo(.12);
    f.issued_at=new Date(now).toISOString();
    expect(communityEffect(v.evaluation,f,24)).toBeNull();
  });
  it("uses the forecast's own community ablation while the independent report lags", () => {
    const v=quantitativeFixture(now),f=forecastFor(v)!;
    const point=f.curve.find(p=>p.hours===24)!;
    point.probability=.971;
    point.without_community_probability=.773;
    expect(communityEffect(v.evaluation,f,24)).toBeCloseTo(.198);
    f.issued_at=new Date(now).toISOString();
    expect(communityEffect(v.evaluation,f,24)).toBeCloseTo(.198);
    expect(communityEffect(null,f,24)).toBeCloseTo(.198);
    delete point.without_community_probability;
    expect(communityEffect(v.evaluation,f,24)).toBeNull();
    expect(communityEffect(null,f,24)).toBeNull();
  });
  it.each([[.62,0],[.72,-.1],[0,.62]])("preserves community ablation %s and its signed effect", (without,effect) => {
    const f=forecastFor(quantitativeFixture(now))!;
    f.curve.find(p=>p.hours===24)!.without_community_probability=without;
    expect(communityEffect(null,f,24)).toBeCloseTo(effect);
  });
  it.each([null,NaN,Infinity,-.1,1.1])("never treats missing or invalid community ablation %s as zero", without => {
    const v=quantitativeFixture(now),f=forecastFor(v)!;
    f.curve.find(p=>p.hours===24)!.without_community_probability=without;
    expect(communityEffect(null,f,24)).toBeNull();
    expect(communityEffect(v.evaluation,f,24)).toBeCloseTo(.12);
    f.issued_at=new Date(now).toISOString();
    expect(communityEffect(v.evaluation,f,24)).toBeNull();
  });
  it("requires the matching legacy ablation version and a real probability", () => {
    const v=quantitativeFixture(now),f=forecastFor(v)!;
    const other=v.evaluation!.latest.find(r=>r.source==="without_community" && r.target===f.target)!;
    other.version="other-version";
    expect(communityEffect(v.evaluation,f,24)).toBeNull();
    other.version=v.evaluation!.latest.find(r=>r.source==="horizon" && r.target===f.target)!.version;
    for(const value of [null,NaN,Infinity,-.1,1.1]) {
      other.probabilities_at_publication["24"]=value;
      expect(communityEffect(v.evaluation,f,24)).toBeNull();
    }
    other.probabilities_at_publication["24"]=0;
    expect(communityEffect(v.evaluation,f,24)).toBeCloseTo(.62);
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
