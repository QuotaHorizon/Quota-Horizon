import type { Language } from "../../i18n";
import type { RadarPoll, RadarPollRound, RadarView } from "./types";
import { EvidenceLink } from "./ArchiveView";
import { publicEvidenceTime } from "./presentation";
import styles from "./resetBriefing.module.less";

const percent = (n: number | null) => n == null ? "—" : `${Number(n.toFixed(1))}%`;
const votes = (n: number, zh: boolean) => `${n} ${zh ? "票" : n === 1 ? "vote" : "votes"}`;
export function timingVoteNodes(round: RadarPollRound) {
  let votes = 0;
  return round.distribution.flatMap(b => {
    votes += b.count;
    return b.deadlineAt && round.samples > 0 ? [{ at: b.deadlineAt, share: 100 * votes / round.samples, votes }] : [];
  });
}
function TimingPoll({ poll, language, now }: { poll: RadarPoll; language: Language; now: number }) {
  const zh = language === "zh"; const r = poll.round;
  const nodes = timingVoteNodes(r);
  const start = Date.parse(r.startsAt); const end = Date.parse(r.endsAt);
  const x = (at: string) => 42 + (Date.parse(at) - start) / (end - start) * 398;
  const y = (p: number) => 148 - p * 1.16;
  const deadline = (t: string) => new Date(t).toLocaleString(zh ? "zh-CN" : "en-GB", { month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit", hour12: false });
  const date = (t: string) => new Date(t).toLocaleDateString(zh ? "zh-CN" : "en-GB", { month: "numeric", day: "numeric" });
  return <div className={styles.pollPanel}>
    <div className={styles.cardHeading}><h3>{zh ? "社区预计什么时候" : "When the community expects it"}</h3><span>{poll.name}</span></div>
    <div className={styles.pollHeadline}><strong>{r.samples}</strong><span>{zh ? "份日期投票" : "timing votes"} · {r.accepting ? (zh ? "本轮进行中" : "Current round") : (zh ? "本轮已结束" : "Round closed")}</span></div>
    {!nodes.length ? <p className={styles.meta}>{zh ? "来源尚未公开这轮票数分布。" : "The source has not published this round’s distribution."}</p> : <>
      <p className={styles.meta}>{zh ? "截至各日期的累计票数占比" : "Cumulative share of votes by deadline"}</p>
      <svg viewBox="0 0 470 190" role="img" aria-label={zh ? "社区日期投票累计分布，按各区间截止时间绘制" : "Cumulative timing votes at each interval deadline"}>
        {[0,50,100].map(n => <g key={n}><line x1="42" x2="440" y1={y(n)} y2={y(n)} className={styles.gridLine} /><text x="34" y={y(n)+4} textAnchor="end">{n}%</text></g>)}
        {now > start && now < end && <g><line x1={x(new Date(now).toISOString())} x2={x(new Date(now).toISOString())} y1="24" y2="151" className={styles.nowLine} /><text x={x(new Date(now).toISOString())} y="16" textAnchor="middle">{zh ? "现在" : "Now"}</text></g>}
        <path className={styles.trendLine} strokeDasharray="4 4" d={nodes.map((p,i) => `${i ? "L" : "M"}${x(p.at)},${y(p.share)}`).join(" ")} />
        {nodes.map(p => <g key={p.at}><circle className={styles.trendDot} cx={x(p.at)} cy={y(p.share)} r="4"><title>{publicEvidenceTime(p.at,language)} · {p.votes}/{r.samples} · {percent(p.share)}</title></circle><text x={x(p.at)} y={y(p.share)-10} textAnchor="middle">{percent(p.share)}</text><text x={x(p.at)} y="177" textAnchor="middle">{date(p.at)}</text></g>)}
      </svg>
      <div className={styles.voteWindows}>{r.distribution.map((b,i) => <div key={i}><span>{b.deadlineAt ? `${i > 0 && r.distribution[i-1].deadlineAt ? `${deadline(r.distribution[i-1].deadlineAt!)} – ` : (zh ? "截至 " : "By ")}${deadline(b.deadlineAt)}` : zh ? "更晚 / 暂不看好近期" : "Later / not soon"}</span><strong>{votes(b.count, zh)}</strong></div>)}</div>
    </>}
    <p className={styles.meta}>{zh ? "票数按原始日期区间保留；曲线标出累计节点。跨越 24h/48h 边界的票暂不指定方向。" : "Votes retain their original date intervals; the curve marks cumulative endpoints. Votes spanning a 24h/48h boundary receive no directional assignment."}</p>
  </div>;
}
function ProbabilityPoll({ poll, language }: { poll: RadarPoll; language: Language }) {
  const zh = language === "zh"; const r = poll.round;
  const rounds = [...poll.recent, r].filter(r => r.meanProbability != null && !r.coverageGap).sort((a,b) => Date.parse(a.startsAt)-Date.parse(b.startsAt));
  const first = rounds[0] ? Date.parse(rounds[0].startsAt) : 0;
  const last = rounds.at(-1) ? Date.parse(rounds.at(-1)!.startsAt) : first;
  const x = (r: RadarPollRound) => 42 + (Date.parse(r.startsAt)-first)/Math.max(last-first,86400_000)*398;
  const y = (r: RadarPollRound) => 148-(r.meanProbability ?? 0)*1.16;
  return <div className={styles.pollPanel}>
    <div className={styles.cardHeading}><h3>{zh ? "社区给出的概率" : "Probabilities chosen by voters"}</h3><span>{poll.name}</span></div>
    <div className={styles.pollHeadline}><strong>{percent(r.meanProbability)}</strong><span>{zh ? `${r.samples} 票的平均判断` : `Mean of ${votes(r.samples, false)}`}</span></div>
    <p className={styles.meta}>{zh ? "本轮截止 " : "Round ends "}{publicEvidenceTime(r.endsAt,language)} · {zh ? "美国太平洋日" : "Pacific calendar day"}</p>
    <svg viewBox="0 0 470 190" role="img" aria-label={zh ? "最近每日轮次的投票平均概率，当前轮次持续更新" : "Mean probability by daily poll round; the current round is ongoing"}>
      {[0,50,100].map(n => <g key={n}><line x1="42" x2="440" y1={148-n*1.16} y2={148-n*1.16} className={styles.gridLine} /><text x="34" y={152-n*1.16} textAnchor="end">{n}%</text></g>)}
      {rounds.map((p,i) => <g key={p.id}>{i>0 && Date.parse(p.startsAt)-Date.parse(rounds[i-1].startsAt)<=25*3600_000 && <path className={styles.pollLine} d={`M${x(rounds[i-1])},${y(rounds[i-1])} L${x(p)},${y(p)}`} />}<circle className={styles.pollDot} cx={x(p)} cy={y(p)} r="4"><title>{publicEvidenceTime(p.startsAt,language)} — {publicEvidenceTime(p.endsAt,language)} · {percent(p.meanProbability)} · {votes(p.samples, zh)}</title></circle><text x={x(p)} y="176" textAnchor="middle">{new Date(p.startsAt).toLocaleDateString(zh ? "zh-CN" : "en-GB",{timeZone:"America/Los_Angeles",month:"numeric",day:"numeric"})}</text></g>)}
    </svg>
    <div className={styles.probabilityBins}>{r.distribution.map(b => <div key={b.probability}><span>{b.probability}%</span><i style={{height:`${r.samples ? 4+22*b.count/r.samples : 4}px`}} /><strong>{votes(b.count, zh)}</strong></div>)}</div>
    <p className={styles.meta}>{zh ? "历史点为各日轮次的最终投票均值，当前点持续更新。每轮预测的截止日不同；匿名浏览器投票，样本较少。" : "Past points are final means for each daily round; the current point updates. Each round has a different deadline. Anonymous browser votes, with a small sample."}</p>
  </div>;
}
export function CommunityForecasts({ radar, language, now, onOpenSource }: { radar: RadarView; language: Language; now: number; onOpenSource?: (url: string) => void }) {
  const zh = language === "zh"; const polls = [...(radar.community.polls ?? [])].sort((a,b) => Number(b.round.kind === "timing") - Number(a.round.kind === "timing"));
  return <article className={styles.card}>
    <div className={styles.cardHeading}><h2>{zh ? "社区量化预期" : "Quantified community outlook"}</h2><span>{zh ? "实际投票 · 日期分布与概率变化" : "Actual ballots · timing and probability trends"}</span></div>
    {polls.length ? <div className={styles.pollGrid}>{polls.map(p => <div key={p.sourceId}>
      {(p.issue || now-Date.parse(p.collectedAt)>=1800_000) && <p className={styles.correction}>{zh ? "采集待更新 · 保留上次投票" : "Update due · saved votes"}</p>}
      {p.round.kind === "timing" ? <TimingPoll poll={p} language={language} now={now} /> : <ProbabilityPoll poll={p} language={language} />}
      <p className={styles.pollSource}><EvidenceLink url={p.url} label={zh ? `投票资料：${p.name}` : `Poll data: ${p.name}`} onOpen={onOpenSource} /><span>{zh ? "读取 " : "Collected "}{publicEvidenceTime(p.collectedAt,language)}</span></p>
    </div>)}</div> : <p className={styles.meta}>{zh ? "等待社区投票接口返回首份数据。" : "Awaiting the first community poll response."}</p>}
    <p className={styles.meta}>{zh ? "直接重置和发卡都可形成额度恢复的乐观预期。投票沿用各站的原始问题与期限；匿名投票合并限制贡献，重复的官方事实不会再作为独立确认。" : "Automatic resets and new reset credits can both support a positive allowance outlook. Polls retain their original questions and deadlines; anonymous ballots share a contribution cap, while repeated official facts are not independent confirmations."}</p>
  </article>;
}

export function CommunityEffectTrend({ radar, language }: { radar: RadarView; language: Language }) {
  const zh = language === "zh";
  const points = radar.community.history ?? [];
  const earliest = points[0] ? Date.parse(points[0].at) : 0;
  const last = points.at(-1) ? Date.parse(points.at(-1)!.at) : earliest;
  const first = Math.min(earliest, last - 1800_000);
  const limit = Math.max(1, ...points.flatMap(p => [Math.abs(p.adjustment24h ?? 0),Math.abs(p.adjustment48h ?? 0)]));
  const x = (t: string) => 38 + (Date.parse(t)-first)/(last-first)*382;
  const y = (n: number) => 61-n/limit*35;
  return <div className={styles.effectTrend}>
    <div className={styles.cardHeading}><h3>{zh ? "社区贡献的变化" : "Community contribution over time"}</h3><span>{zh ? "24h 实线 · 48h 虚线" : "24h solid · 48h dashed"}</span></div>
    {!points.length ? <p className={styles.meta}>{zh ? "新口径的贡献走势从首次采集开始记录。" : "The updated contribution series begins with the first collection."}</p> : <svg viewBox="0 0 450 125" role="img" aria-label={zh ? "社区对 24h 和 48h 综合概率的贡献，单位为百分点" : "Community contribution to 24h and 48h estimates, in percentage points"}>
      {[-limit,0,limit].map(n => <g key={n}><line x1="38" x2="420" y1={y(n)} y2={y(n)} className={styles.gridLine} /><text x="31" y={y(n)+3} textAnchor="end">{n>0?"+":""}{Number(n.toFixed(1))}</text></g>)}
      {([24,48] as const).map(h => <g key={h}>{points.map((p,i) => {
        const value = h===24?p.adjustment24h:p.adjustment48h; if(value==null)return null;
        const prior=points[i-1]; const old=prior?(h===24?prior.adjustment24h:prior.adjustment48h):null;
        return <g key={p.at}>{prior&&old!=null&&p.modelVersion===prior.modelVersion&&Date.parse(p.at)-Date.parse(prior.at)<45*60_000&&<path className={styles.trendLine} strokeDasharray={h===48?"5 4":undefined} d={`M${x(prior.at)},${y(old)} L${x(p.at)},${y(value)}`} />}<circle className={styles.trendDot} cx={x(p.at)} cy={y(value)} r="2.5"><title>{h}h · {value>0?"+":""}{value} · {publicEvidenceTime(p.at,language)}</title></circle></g>;
      })}</g>)}
      <text x="38" y="120">{new Date(first).toLocaleTimeString(zh?"zh-CN":"en-GB",{hour:"2-digit",minute:"2-digit",hour12:false})}</text><text x="420" y="120" textAnchor="end">{new Date(last).toLocaleTimeString(zh?"zh-CN":"en-GB",{hour:"2-digit",minute:"2-digit",hour12:false})}</text>
    </svg>}
  </div>;
}
