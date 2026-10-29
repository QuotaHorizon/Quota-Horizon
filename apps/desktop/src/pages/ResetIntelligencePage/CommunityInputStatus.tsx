import type { Language } from "../../i18n";
import type { CommunityDiagnostic, QuantForecast } from "./quantitativeTypes";
import { publicEvidenceTime } from "./presentation";
import styles from "./quantitative.module.less";

const reasons: Record<string, [string, string]> = {
  source_unavailable: ["等待来源数据", "Awaiting source data"],
  forecast_unavailable: ["等待预测恢复", "Awaiting a usable forecast"],
  no_current_poll: ["来源暂无投票", "No poll from this source"],
  target_mismatch: ["问题范围不同", "Different event scope"],
  observation_stale: ["采样已过期", "Observations are stale"],
  source_quality: ["数据质量未达标", "Data quality check failed"],
  empty_poll: ["尚无投票", "No votes yet"],
  no_recent_votes: ["近期无新增票", "No newly observed ballots"],
  vote_timing_unavailable: ["新增票时间待确认", "Vote arrival times unavailable"],
  insufficient_recent_votes: ["近期样本不足", "Recent sample too small"],
  no_usable_current_poll: ["当前窗口不可用", "No usable current window"],
  duplicate_poll_round: ["同一轮投票已计入", "This poll round is already included"],
  poll_window_closed: ["投票期限已结束", "Poll window ended"],
  poll_window_upcoming: ["投票期限尚未开始", "Poll window has not started"],
  unreviewed_calendar_question: ["问题口径待确认", "Question scope needs review"],
  calendar_window_closed_or_upcoming: ["投票已关闭或尚未开始", "Poll closed or not yet open"],
  calendar_event_already_observed: ["该轮重置已发生", "This round’s reset already occurred"],
  calendar_completion_time_unresolved: ["重置完成时间待核实", "Completion time needs verification"],
  calendar_sampling_gap: ["采样间断过长", "Sampling gap too long"],
  unreviewed_calendar_units: ["投票单位待确认", "Ballot units need review"],
  calendar_vote_timing_unavailable: ["新增票时间待确认", "Vote arrival times unavailable"],
};

export function communityQuestion(d: CommunityDiagnostic, language: Language) {
  const zh = language === "zh";
  return d.question_kind === "calendar_preference" ? (zh ? "重置 / 新功能偏好与预期" : "Reset / feature preference and expectation")
    : d.question_kind === "interval_votes" ? (zh ? "重置时间预期" : "Expected reset timing")
    : d.question_kind === "probability_votes" ? (zh ? "期限内重置概率" : "Reset probability within the poll window") : null;
}

export function CommunityInputStatus({forecast, language}: {forecast: QuantForecast; language: Language}) {
  const zh = language === "zh";
  const rows = forecast.community_diagnostics?.filter(d => d.target === forecast.target && d.state === "excluded") ?? [];
  if (!rows.length) return null;
  const time = (at: number) => publicEvidenceTime(new Date(at * 1000).toISOString(), language);
  return <div className={styles.ballotStatus} aria-label={zh ? "未采用的投票" : "Excluded ballots"}>
    {rows.map(d => <div key={d.id}>
      <div><b>{d.id === "didcodex_poll" ? "Did Codex Reset" : d.id === "date_poll" ? "Codex Reset" : "NextReset"}</b><span>{(reasons[d.reason ?? ""] ?? reasons.source_quality)[zh ? 0 : 1]}</span></div>
      <small>{zh ? "近 6h 新增 " : "New in 6h: "}<b>{d.recent_votes ?? "—"}</b> / {d.minimum_recent_votes} {d.unit === "clicks" ? (zh ? "次点击门槛" : "click threshold") : (zh ? "票门槛" : "vote threshold")}{d.total_votes != null && (zh ? ` · 累计 ${d.total_votes}` : ` · ${d.total_votes} total`)}</small>
      {communityQuestion(d, language) && <small>{communityQuestion(d, language)}</small>}
      {d.window_start != null && d.window_end != null && <small>{zh ? "原期限 " : "Original window "}{time(d.window_start)} – {time(d.window_end)}</small>}
      <small>{d.read_failed ? (zh ? "最近读取失败" : "Latest read failed") : (zh ? "最近采样" : "Last observed")}{d.observed_at != null ? ` · ${time(d.observed_at)}` : " —"}{d.latest_vote_at != null && (zh ? ` · 新增票 ${time(d.latest_vote_at)}` : ` · New ballots ${time(d.latest_vote_at)}`)}</small>
    </div>)}
  </div>;
}
