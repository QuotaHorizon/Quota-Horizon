import { useMemo, useState } from "react";
import { LineChart } from "echarts/charts";
import { AriaComponent, GridComponent, TooltipComponent } from "echarts/components";
import { use } from "echarts/core";
import { CanvasRenderer } from "echarts/renderers";
import type { DesktopHistoryEnvelope } from "../../../../capacity-preview/src/status";
import type { Language } from "../../i18n";
import { quotaPercentLabel } from "../../utils/quotaPercent";
import { historyEmptyMessage } from "./presentation";
import { historyChartOption, historyRangePoints, historySeries, type HistoryRange } from "./historyChart";
import { CapacityHistoryHeatmap } from "./CapacityHistoryHeatmap";
import { EChart } from "../../components/TokenUsageDashboard/EChart";
import styles from "./index.module.less";

use([LineChart, AriaComponent, GridComponent, TooltipComponent, CanvasRenderer]);

export function CapacityHistory({ history, language, dark = false, onAuthorize, authorizing = false }: {
  history: DesktopHistoryEnvelope | null;
  language: Language;
  dark?: boolean;
  onAuthorize?: () => void;
  authorizing?: boolean;
}) {
  const [range, setRange] = useState<HistoryRange>("7d");
  const allPoints = useMemo(() => historySeries(history?.points ?? []).flat(), [history?.points]);
  const points = useMemo(() => historyRangePoints(allPoints, range), [allPoints, range]);
  const option = useMemo(() => historyChartOption(points, language, dark), [points, language, dark]);
  if (history?.status !== "available" || !points.length) {
    return <div className={styles.chartEmpty} role="status">
      <div>{historyEmptyMessage(history, language)}
        {onAuthorize && ["history_keychain_denied", "history_keychain_interaction_required"].includes(history?.reasonCode ?? "") && <>
          <button className={styles.historyAuthorize} type="button" disabled={authorizing} onClick={onAuthorize}>
            {authorizing ? (language === "zh" ? "等待系统授权…" : "Waiting for authorization…") : (language === "zh" ? "授权本地历史密钥" : "Authorize local history key")}
          </button>
          <small>{language === "zh"
            ? "请在 macOS 提示中选择「始终允许」，以便持续保存历史。"
            : "Choose Always Allow in the macOS prompt to keep recording history."}</small>
          {import.meta.env.VITE_HORIZON_SIGNING_MODE === "ad_hoc" && <details><summary>{language === "zh" ? "为什么更新后需要授权？" : "Why authorize after an update?"}</summary><small>{language === "zh"
            ? "本地构建更新后，macOS 可能要求重新授权。授权后即可继续查看和保存历史。"
            : "macOS may request authorization again after a local build update. Authorize to view and record history."}</small></details>}
        </>}
      </div>
    </div>;
  }
  const latest = points[points.length - 1];
  const flat = points.length > 1 && points.every((point) => point.remainingPercent === latest.remainingPercent);
  return <>
    <div className={styles.historyToolbar}>
      <div className={styles.historyRanges} role="group" aria-label={language === "zh" ? "历史时间范围" : "History time range"}>
        {(["24h", "7d", "30d"] as const).map((value) => <button key={value} type="button" aria-pressed={range === value} onClick={() => setRange(value)}>
          {language === "zh" ? ({ "24h": "24 小时", "7d": "7 天", "30d": "30 天" })[value] : value === "24h" ? "24 hours" : value === "7d" ? "7 days" : "30 days"}
        </button>)}
      </div>
    </div>
    <div className={styles.chart}>
      <EChart option={option} label={language === "zh" ? `额度历史，${points.length} 个样本` : `Capacity history, ${points.length} samples`} />
    </div>
    <div className={styles.chartSummary}>
      {history.overview
        ? (language === "zh" ? `${points.length} 个绘图点 · 最新剩余 ` : `${points.length} plotted observations · Latest remaining `)
        : (language === "zh" ? `${points.length} 个本地样本 · 最新剩余 ` : `${points.length} local samples · Latest remaining `)}
      <strong>{quotaPercentLabel(latest.remainingPercent)}</strong>
      {flat && <span>{language === "zh" ? " · 所选时段内已记录的额度没有变化" : " · No recorded change in this range"}</span>}
    </div>
    <div className={styles.historyLegend} aria-label={language === "zh" ? "历史连线图例" : "History line legend"}>
      <span><i aria-hidden="true" />{language === "zh" ? "实线：下降 / 持平" : "Solid: decrease / unchanged"}</span>
      <span><i data-dashed aria-hidden="true" />{language === "zh" ? "虚线：额度回升" : "Dashed: quota rise"}</span>
    </div>
    <CapacityHistoryHeatmap history={history} latest={latest.capturedAt} language={language} />
  </>;
}
