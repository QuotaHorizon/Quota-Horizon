import { useEffect, useMemo, useSyncExternalStore } from "react";
import { getCapacityDemandPlan, isDesktopApp, subscribeToCapacityDemandPlan, updateCapacityDemandPlan } from "../../api/backend";
import type { Language } from "../../i18n";
import { CapacityDemandView } from "./CapacityDemandView";
import { CapacityActiveTimePanel } from "./CapacityActiveTimePanel";
import { PlanningArchivePanel } from "./PlanningArchivePanel";
import { CapacityPacePanel } from "./CapacityPacePanel";
import { createDemandPlanStore } from "./demandPlanStore";
import styles from "./CapacityDemand.module.less";

interface Props { contextId?: string; sequence: number; language: Language; compact?: boolean; onOpen?: () => void }

export function CapacityDemandPanel(props: Props) {
  // Keyed mount clears unsaved input immediately when the account changes,
  // not one effect later. A response from the old view stays in its old store.
  if (!isDesktopApp || !props.contextId) return <section className={`${styles.panel} ${props.compact ? styles.compact : ""}`}><div className={styles.heading}><h3>{props.language === "zh" ? "我的工作需求" : "My work demand"}</h3></div><p className={styles.muted}>{props.language === "zh" ? "等待当前账号的本地历史绑定。旧计划保留在本机。" : "Waiting for this account’s local history binding. Existing plans are retained on this device."}</p></section>;
  return <BoundDemandPanel key={props.contextId} {...props} contextId={props.contextId} />;
}

function BoundDemandPanel({ contextId, sequence, language, compact, onOpen }: Props & { contextId: string }) {
  const store = useMemo(() => createDemandPlanStore(contextId, { read: getCapacityDemandPlan, update: updateCapacityDemandPlan, subscribe: subscribeToCapacityDemandPlan }), [contextId]);
  const snapshot = useSyncExternalStore(store.subscribe, store.getSnapshot, store.getSnapshot);
  useEffect(() => { void store.reload(); }, [store, sequence]);
  useEffect(() => {
    const reload = () => { if (document.visibilityState === "visible") void store.reload(); };
    window.addEventListener("focus", reload);
    document.addEventListener("visibilitychange", reload);
    // Recompute deadline/allocation on the local clock; this reads local state
    // and does not request a quota-network refresh.
    const timer = window.setInterval(reload, 30_000);
    return () => { window.clearInterval(timer); window.removeEventListener("focus", reload); document.removeEventListener("visibilitychange", reload); };
  }, [store]);
  return <>
    <CapacityDemandView {...snapshot} language={language} contextId={contextId} compact={compact}
      onOpen={onOpen} onSave={store.save} onRetry={() => void store.reload()}
      renderArchive={(onUse, canUse) => <PlanningArchivePanel contextId={contextId} sequence={sequence}
        language={language} onUse={onUse} canUse={canUse} />} />
    <CapacityActiveTimePanel contextId={contextId} sequence={sequence} language={language} compact={compact} onOpen={onOpen} />
    {!compact && <CapacityPacePanel contextId={contextId} sequence={sequence} language={language} />}
  </>;
}
