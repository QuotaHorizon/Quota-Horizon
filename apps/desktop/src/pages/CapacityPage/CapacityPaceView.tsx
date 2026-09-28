import type {DesktopPaceEnvelope, PaceEstimate, PaceRange} from "../../../../capacity-preview/src/status";
import type {Language} from "../../i18n";
import {planTime} from "./demandPlanModel";
import styles from "./CapacityPace.module.less";

export function paceMessage(code: string, language: Language): string {
  const zh = language === "zh";
  const messages: Record<string, [string, string]> = {
    plan_context_changed: ["账号或 Codex 版本已变化，正在更新。", "The account or Codex version changed. Updating."],
    plan_binding_required: ["先完成当前账号的本地历史绑定，才能积累独立样本并推算。", "Bind local history for this account before collecting its own samples and estimating pace."],
    plan_store_unavailable: ["历史暂不可用，请稍后重试。", "History is unavailable. Try again later."],
    pace_read_failed: ["读取本地历史失败，暂时不显示推算数值，请重试。", "Could not read local history. No scenario numbers are shown; please retry."],
    plan_context_missing: ["旧记录缺少当时的套餐信息，需要重新积累样本。", "Older records lack plan-at-capture information. New samples are needed."],
    plan_type_changed: ["历史中的套餐发生变化，需要变化之后的新基线。", "The plan changed within this history. A new post-change baseline is needed."],
    insufficient_history: ["同一账号、读取器和额度窗口的连续历史还不够。保持自动刷新即可继续积累，无需手动反复刷新。", "There is not enough continuous history for this account, reader and window. Automatic refresh will collect it; repeated manual refresh is unnecessary."],
    insufficient_usage: ["近期额度变化较少，暂时无法估算。", "Recent quota changes are too small to estimate pace."],
    unstable_pace: ["最近三个时段的消耗速度相差过大，暂不把它们延伸为未来结果。", "Consumption rates vary too much across the last three blocks to extend them forward."],
    observation_gap: ["采样间隔过大或边界缺少读数，不跨越休眠/失联缺口推算。", "Capture gaps or missing boundary samples prevent an estimate across sleep or disconnected periods."],
    balance_increased: ["额度近期回升，需要积累回升后的样本。", "Quota recently rose. More post-rise samples are needed."],
    window_changed: ["历史跨过额度窗口或重置边界，需要当前周期内的新基线。", "This history crosses a window or reset boundary. A baseline within the current cycle is needed."],
    window_ended: ["窗口或推算时点已经到期，等待新的额度后重新计算。", "The window or scenario horizon has ended. Waiting for a new balance."],
    stale_source: ["最新读数已超过 30 分钟，暂不展示推算范围。", "The latest capture is over 30 minutes old; no scenario range is shown."],
    current_capture_pending: ["当前读数与已存快照尚未一致，等待本地采集同步。", "The current balance and stored capture are not yet aligned. Waiting for local capture synchronization."],
    conflicting_samples: ["同一时刻存在冲突读数，暂不选择其中一个推算。", "Conflicting values share a timestamp; none is selected arbitrarily."],
    clock_discontinuity: ["读数时间晚于当前电脑时间，暂不计算。", "A capture is later than the computer’s current time; no estimate is calculated."],
    source_unavailable: ["额度来源或兼容性状态不满足本次推算条件。", "The quota source or compatibility state does not meet this estimate’s requirements."],
    unsupported_window: ["这个额度窗口暂不适用于当前实验算法。", "This quota window is not supported by the experimental algorithm."],
    missing_reset_time: ["尚无确切重置时间，无法约束推算范围。", "A reset time is required to bound the scenario horizon."],
    balance_empty: ["当前额度已为零，无需继续推算耗尽时间。", "The observed balance is already zero; no future depletion time is estimated."],
    query_truncated: ["本次原始历史达到读取上限，未使用截断数据推算。", "The raw-history read limit was reached; truncated inputs are not used."],
  };
  if (code.startsWith("history_keychain")) return zh ? "本地历史密钥尚未授权，旧记录仍保留；请先在额度历史中授权。" : "Local history access is not authorized. Earlier captures are retained; authorize in Capacity history first.";
  return messages[code]?.[zh ? 0 : 1] ?? (zh ? "当前样本暂不满足推算条件，等待可用的账号额度和历史。" : "Current samples do not support a scenario. Waiting for usable account quota and history.");
}
const number = (value: number, language: Language) => value > 0 && value < .1 ? "<0.1" : value.toLocaleString(language === "zh" ? "zh-CN" : "en-US", {maximumFractionDigits: 1});
export function paceRangeLabel(range: PaceRange, language: Language): string {
  if (range.status !== "available" || range.lower == null || range.upper == null || !Number.isFinite(range.lower) || !Number.isFinite(range.upper)) return "—";
  const lower = number(range.lower, language), upper = number(range.upper, language);
  return lower === upper ? `≈ ${lower}` : `${lower}–${upper}`;
}
export function paceIsCurrent(value: PaceEstimate, now: number) {
  const observed = Date.parse(value.observedAt ?? ""), until = Date.parse(value.forecastHorizon ?? "");
  return value.state === "pace_only" && Number.isFinite(observed) && now >= observed && now - observed <= 1_800_000 && Number.isFinite(until) && until > now;
}
function Window({value, language, failed, now}: {value: PaceEstimate; language: Language; failed: boolean; now: number}) {
  const zh = language === "zh", current = !failed && paceIsCurrent(value, now);
  const observed = Date.parse(value.observedAt ?? ""), until = Date.parse(value.forecastHorizon ?? "");
  const staleReason = !Number.isFinite(observed) || !Number.isFinite(until) ? "source_unavailable" : observed > now ? "clock_discontinuity" : until <= now ? "window_ended" : "stale_source";
  const title = value.windowMinutes === 10080 ? (zh ? "每周额度" : "Weekly quota") : value.windowMinutes === 300 ? (zh ? "5 小时额度" : "5-hour quota") : value.windowMinutes ? `${number(value.windowMinutes / 60, language)} ${zh ? "小时窗口" : "hour window"}` : (zh ? "额度窗口" : "Quota window");
  const coverage = value.historyCoverage;
  return <article className={styles.window}>
    <div className={styles.heading}><strong>{title}</strong><small>{zh ? "实验 · 未校准" : "Experimental · uncalibrated"}</small></div>
    {current ? <>
      <span className={styles.balance}>{paceRangeLabel(value.balanceAtHorizon, language)}%</span>
      <p>{zh ? "若保持近期速率，预计在 " : "If recent rates continue, the balance at "}{planTime(value.forecastHorizon, language)}{zh ? " 的剩余额度情景范围（不跨下一次重置）。" : " falls within this scenario band (without crossing the next reset)."}</p>
      <p className={styles.muted}>{zh ? "依据最近三个时段的 " : "Based on the last three blocks: "}{paceRangeLabel(value.rateRange, language)} {zh ? "百分点 / 日历小时。" : "percentage points / elapsed hour."}</p>
      {value.depletionTimeRange.status === "available" ? <p>{zh ? "同一假设下可能耗尽于 " : "Under the same assumption, depletion falls between "}{planTime(value.depletionTimeRange.earliestAt, language)} → {planTime(value.depletionTimeRange.latestAt, language)}</p> : <p className={styles.muted}>{zh ? "暂时无法估算耗尽时间。" : "Depletion time is unavailable."}</p>}
    </> : <p>{failed ? (zh ? "本次读取失败，暂时撤下推算数值；请重试。" : "This read failed. Scenario numbers are hidden until a successful retry.") : paceMessage(value.state === "pace_only" ? staleReason : value.reasonCode, language)}</p>}
    <div className={styles.metrics}><span>{zh ? `原始采样 ${coverage.sampleCount} 个` : `${coverage.sampleCount} original captures`}</span><span>{zh ? `当前覆盖 ${number(coverage.coveredSeconds / 3600, language)} 小时 / 目标 ${number(coverage.requiredSeconds / 3600, language)} 小时` : `${number(coverage.coveredSeconds / 3600, language)} hours covered / ${number(coverage.requiredSeconds / 3600, language)} required`}</span>{coverage.maximumGapSeconds > 0 && <span>{zh ? `最大间隔 ${number(coverage.maximumGapSeconds / 60, language)} 分钟` : `Largest gap ${number(coverage.maximumGapSeconds / 60, language)} min`}</span>}</div>
    {value.observedAt && <small>{zh ? "基于采集于 " : "Capture used: "}{planTime(value.observedAt, language)}{value.cachedSource ? (zh ? " · 缓存读数" : " · cached capture") : ""}</small>}
    {value.compatibilityUnverified && <p className={styles.muted}>{zh ? "读取器兼容性待验证。" : "Reader compatibility is unverified."}</p>}
  </article>;
}
export function CapacityPaceView({value, contextId, language, failed = false, loading = false, onRetry, now = Date.now()}: {value: DesktopPaceEnvelope | null; contextId: string; language: Language; failed?: boolean; loading?: boolean; onRetry: () => void; now?: number}) {
  const zh = language === "zh", available = value?.historyContextId === contextId && value.status === "available";
  return <section className={styles.panel} aria-label={zh ? "近期节奏" : "Recent pace"}>
    <div className={styles.heading}><h3>{zh ? "近期节奏" : "Recent pace"}</h3><button disabled={loading} onClick={onRetry}>{loading ? (zh ? "计算中…" : "Calculating…") : (zh ? "重新计算" : "Recalculate")}</button></div>
    <p className={styles.intro}>{zh ? "按最近三个时段的消耗速度估算短期余额，结果为实验估算。" : "Experimental short-term balance estimates based on consumption across the last three blocks."}</p>
    {!value ? <p>{failed ? (zh ? "读取暂时失败，请重试。" : "Could not read local captures. Please retry.") : (zh ? "正在检查本地原始采样…" : "Checking original local captures…")}</p> : !available ? <p>{paceMessage(value.historyContextId !== contextId ? "plan_context_changed" : value.reasonCode, language)}</p>
      : <>{!value.estimates.length ? <p>{zh ? "当前没有可用于推算的官方固定额度窗口。" : "There is no official fixed quota window to estimate."}</p> : <div className={styles.windows}>{value.estimates.map(estimate => <Window key={estimate.limitId} value={estimate} language={language} failed={failed} now={now}/>)}</div>}</>}
    <details className={styles.explanation}><summary>{zh ? "计算方法" : "Calculation method"}</summary><p>{zh ? "将近期最快和最慢消耗速度延伸至当前周期内。使用方式或套餐变化后需要重新计算。" : "Extends the fastest and slowest recent consumption rates within the current cycle. Changes in usage or plan require a new calculation."}</p><p>{zh ? "范围按日历时间计算，准确性尚待校准。" : "The range uses elapsed time. Accuracy calibration is still pending."}</p></details>
  </section>;
}
