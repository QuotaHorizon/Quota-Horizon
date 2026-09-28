import { memo, useEffect, useState } from "react";
import { History } from "lucide-react";
import { getCapacityHistory, hasLocalBackend } from "../../api/backend";
import type { Language } from "../../i18n";
import { CapacityHistory } from "./CapacityHistory";
import { settleHistory, visibleHistory, type HistoryView } from "./historyState";
import styles from "./index.module.less";

export const CapacityHistoryPanel = memo(function CapacityHistoryPanel({
  contextId, executableId, limitId, sequence, language, dark, onAuthorize, authorizing,
}: {
  contextId?: string; executableId?: string | null; limitId?: string; sequence: number;
  language: Language; dark: boolean; onAuthorize?: () => void; authorizing: boolean;
}) {
  // Legacy hosts without a context cannot safely retain history across updates.
  const key = JSON.stringify([contextId ?? `legacy-${sequence}`, executableId, limitId]);
  const [view, setView] = useState<HistoryView>({ key, history: null, issue: null });
  const current = visibleHistory(view, key);
  const displayedHistory = !limitId && sequence > 0 ? {
    schemaVersion: "1.0" as const, status: "unavailable" as const,
    reasonCode: "history_context_unavailable", points: [],
  } : current.history;
  useEffect(() => {
    if (!hasLocalBackend || !limitId || !sequence) return;
    let active = true;
    void getCapacityHistory(limitId).then((next) => {
      if (active) setView((previous) => settleHistory(previous, key, contextId, next));
    }).catch(() => {
      if (active) setView((previous) => settleHistory(previous, key, contextId, {
        schemaVersion: "1.0", historyContextId: contextId, status: "failed",
        reasonCode: "history_query_failed", points: [],
      }));
    });
    return () => { active = false; };
  }, [contextId, key, limitId, sequence]);

  const format = (value?: string) => value && Number.isFinite(Date.parse(value))
    ? new Intl.DateTimeFormat(language === "zh" ? "zh-CN" : "en-US", {
      month: "short", day: "numeric", hour: "2-digit", minute: "2-digit", hourCycle: "h23",
    }).format(new Date(value)) : "—";
  const points = current.history?.points ?? [];
  return <section className={styles.historyCard}>
    <div className={styles.sectionHeading}>
      <span><History size={17} /><h3>{language === "zh" ? "额度历史" : "Capacity history"}</h3></span>
      <small>{language === "zh" ? "按账户保存 · 后台自动更新" : "Account-bound · Updates in the background"}</small>
    </div>
    <CapacityHistory history={displayedHistory} language={language} dark={dark}
      onAuthorize={onAuthorize} authorizing={authorizing} />
    <div className={styles.chartFooter}>
      <span>{language === "zh" ? "最早" : "Oldest"}: {format(points[0]?.capturedAt)}</span>
      <span>{current.issue
        ? (current.history?.status === "available"
          ? (language === "zh" ? "更新暂不可用 · 保留上次结果" : "Update unavailable · Previous result retained")
          : (language === "zh" ? "等待当前账号数据" : "Waiting for the current account"))
        : `${language === "zh" ? "最新" : "Latest"}: ${format(points.at(-1)?.capturedAt)}`}</span>
    </div>
  </section>;
});
