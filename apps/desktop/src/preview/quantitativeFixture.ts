// Synthetic UI/test data only; never imported by the installed app entry point.
import type { QuantForecast, QuantitativeView, QuantReport, QuantScorecard, ResetTarget } from "../pages/ResetIntelligencePage/quantitativeTypes";

// Discrete synthetic publications: quiet periods, separate source updates and
// abrupt revisions. Never add periodic waves or interpolate extra changes.
// Each offset is relative to the source's final probability, in elapsed minutes.
const receivedChanges: Record<string, [number, number][]> = {
  horizon: [[0,-.28],[65,-.27],[115,-.29],[190,-.22],[260,-.21],[345,-.19],
    [425,-.03],[430,.10],[445,.08],[455,.12],[565,.13],[585,.11],[620,.105],
    [690,.09],[735,.08],[780,-.23],[790,-.235],[845,-.22],[910,-.21],
    [990,-.17],[1015,-.16],[1060,-.10],[1070,-.11],[1095,-.12],
    [1210,-.10],[1290,-.02],[1305,.03],[1310,.02],[1370,0]],
  horizon_auto: [[0,-.11],[185,-.105],[430,-.09],[780,-.15],[975,-.13],
    [1215,-.08],[1280,-.02],[1315,0]],
  codex_reset: [[0,0]],
  monitor: [[0,-.10],[195,-.09],[415,-.08],[785,-.12],[1020,-.10],[1280,0]],
  nextreset: [[0,-.30],[120,-.28],[300,-.25],[480,-.23],[660,-.20],
    [840,-.17],[1020,-.13],[1200,-.07],[1320,-.03],[1395,0]],
  lunar: [[0,-.12],[420,-.085],[780,-.14],[1080,-.11],[1285,0]],
  observatory: [[0,.09],[300,.11],[620,.08],[900,.04],[1100,.02],[1325,0]],
};

export function quantitativeFixture(now: number, mature = false): QuantitativeView {
  const stamp = new Date(now-60_000).toISOString(), expiry = new Date(now+14*60_000).toISOString();
  const curve = (p: number) => Array.from({length:72},(_,i)=>1-Math.pow(1-p,(i+1)/24));
  const forecasts: QuantForecast[] = (["broad_quota_relief","automatic_reset"] as ResetTarget[]).map((target,i) => {
    const values = curve(i ? .44 : .62), base = curve(.32);
    return { target, status:"current", issued_at:stamp,valid_until:expiry,model_version:"synthetic-model",training_state:"cold_start_prior",evaluation_windows:0,event_count:57-i*7,
      curve:values.map((probability,j)=>({hours:j+1,probability,baseline:base[j],component_range:[.2,.8]})),
      sources:(i?["codex_reset","monitor","nextreset"]:["observatory","lunar"]).map((id,j)=>({id,probabilities:curve(.22+j*.16),source_points:[[24,.22+j*.16],[48,.6]],origin:(now-60_000)/1000,source_coverage_hours:47.8})),
      community:{probabilities:curve(i?.52:.72),weight:.5} };
  });
  for (const f of forecasts) f.changes = {since:new Date(now-6*3600_000).toISOString(), until:stamp, new_statement_ids:[],
    windows:[24,48].map(hours => { const after=f.curve[hours-1].probability; return {hours,before:after-.12,after,delta:.12,coherence_effect:0,
      drivers:[{id:"community",before:.48,after:.72,weight_before:.5,weight_after:.5,raw_before:null,raw_after:null,input_effect:.12,weight_effect:0,effect:.12}]}; })};
  const version = "synthetic-model:fixture-config";
  const report: QuantReport = { schema_version:"reset-observer/1",generated_at:stamp,started_at:new Date(now-7*3600_000).toISOString(),state:mature?"scoring":"collecting_forward_evidence",
    policy:{reporting_grace_seconds:10800,minimum_rank_windows:30,minimum_rank_events:5},
    summary:{forecast_snapshots:637,frozen_trials:304,scored_nonoverlapping_trials:mature?32:0,settlements:mature?{settled:32,pending:260,unresolved:12}:{pending:304},next_maturity:new Date(now+2*3600_000).toISOString(),settlement_revisions:304},
    latest:[],curves:[],scorecards:[],comparisons:[],outcomes:[],outcome_providers:[{provider:"completion_feed",last_attempt:stamp,generated_at:stamp,healthy:true},{provider:"public_catalogue",last_attempt:stamp,generated_at:stamp,healthy:true},{provider:"quota_events",last_attempt:stamp,generated_at:new Date(now-2*3600_000).toISOString(),healthy:false}] };
  for (const f of forecasts) {
    const identities = [{source:"horizon",track:"model" as const,probabilities:f.curve.map(p=>p.probability)},...f.sources.map(s=>({source:s.id,track:"native" as const,probabilities:s.probabilities}))];
    for (const [index,identity] of identities.entries()) {
      const {source,track,probabilities:p} = identity;
      report.latest.push({source,track,version,target:f.target,origin:stamp,received_at:stamp,valid_until:expiry,state:"current",probabilities_at_publication:{"6":p[5],"12":p[11],"24":p[23],"48":p[47]}});
      for (const hours of [24,48]) {
        // Cover the complete visible day with discrete synthetic receipts.
        // Flat values remain flat; the final value matches the forecast above.
        const interval = hours === 24 ? 5 * 60_000 : 3600_000;
        const end = Math.floor((now - 60_000) / interval) * interval;
        const start = hours === 24 ? Math.floor((now - 86400_000) / 1800_000) * 1800_000 : end - 6 * interval;
        const count = Math.floor((end - start) / interval) + 1;
        const changes = receivedChanges[source === "horizon" && f.target === "automatic_reset" ? "horizon_auto" : source];
        report.curves.push({source,track,version,target:f.target,hours,
          ...(source === "horizon" && hours === 24 ? {candle_interval_minutes:30} : {}),
          points:Array.from({length:count},(_,i)=>{
            const progress = i / (count - 1);
            const minute = i * interval / 60_000;
            const offset = hours === 24 ? changes.filter(([at]) => at <= minute).at(-1)![1] : -(1-progress)*.162;
            const gap = index === 1 && (hours === 24 ? minute >= 615 && minute <= 655 : i === 3);
            return {at:new Date(start + i * interval).toISOString(),
              p:gap ? null : Math.max(0,Math.min(1,p[hours-1]+offset))};
          })});
        const row: QuantScorecard = {source,track,version,target:f.target,hours,scheduled_windows:16,forecast_coverage:1,settlement_states:mature?{settled:8,pending:8}:{pending:16},
          scores:{n:mature?8:0,unique_events:mature?2:0,brier:mature?.184:null,log_loss:mature?.526:null,state:mature?"insufficient_events_or_windows":"awaiting_mature_windows",brier_interval:{mean:mature?.184:null,ci95:null,blocks:mature?2:0},calibration:mature?[{n:5,mean_probability:.2,event_frequency:.2},{n:3,mean_probability:.65,event_frequency:.67}]:[]},
          alerts:{threshold:.7,episodes:1,false_alerts:0,precision:mature?1:null,event_recall:mature?.5:null,median_lead_hours:mature?9.5:null}};
        report.scorecards.push(row);
      }
    }
    report.latest.push({source:"without_community",track:"shadow",version,target:f.target,origin:stamp,received_at:stamp,valid_until:expiry,state:"current",probabilities_at_publication:{"6":.1,"12":.2,"24":.5,"48":.7}});
  }
  report.outcomes = ["confirmed","pending","targeted","disputed"].map((status,i)=>({id:`synthetic-outcome-${i}`,status,kind:i%2?"automatic":"banked",from:(now-(i+1)*86400_000)/1000,to:status==="pending"?null:(now-(i+1)*86400_000)/1000,announced_at:(now-(i+1)*86400_000)/1000,known_at:now/1000,references:["https://example.com/synthetic-event"],primary_references:1,completion_reports:2}));
  if(mature) report.comparisons.push({left:{source:"horizon",version,track:"model"},right:{source:"without_community",version,track:"shadow"},target:"broad_quota_relief",hours:24,n:8,unique_events:2,brier_skill:.08,loss_difference:{mean:-.02,ci95:null,blocks:2},error_correlation:.92});
  return {connected:true,checkedAt:stamp,forecastIssue:null,evaluationIssue:null,forecast:{schema_version:"reset-intelligence-consumer/1",forecasts},evaluation:report};
}
