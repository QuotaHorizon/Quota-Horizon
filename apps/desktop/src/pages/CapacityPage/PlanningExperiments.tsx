import { useEffect, useRef, useState, type ReactNode } from "react";
import type { Language } from "../../i18n";
import { getCapacityStatus, getCapacityStatusSnapshot, subscribeToCapacityStatus } from "../../api/backend";
import { acceptNewerEnvelope } from "../../components/CapacityPopover/presentation";
import { CapacityDemandPanel } from "./CapacityDemandPanel";
import styles from "./index.module.less";

/** Closed experiments are unmounted: no history queries, timers or trial writes. */
export function PlanningExperiments({ language, children, focusRequest = 0, onFocusHandled }: {
  language: Language; children: ReactNode; focusRequest?: number; onFocusHandled?: () => void;
}) {
  const [open, setOpen] = useState(false);
  const disclosure = useRef<HTMLDetailsElement>(null);
  const handled = useRef(0);
  useEffect(() => {
    if (!focusRequest) { handled.current = 0; return; }
    if (handled.current === focusRequest) return;
    handled.current = focusRequest;
    setOpen(true);
    disclosure.current?.scrollIntoView({ block: "center" });
    disclosure.current?.focus({ preventScroll: true });
    onFocusHandled?.();
  }, [focusRequest, onFocusHandled]);
  return <details ref={disclosure} tabIndex={-1} className={styles.experimentsDisclosure}
    open={open} onToggle={(event) => setOpen(event.currentTarget.open)}>
    <summary>{language === "zh" ? "规划实验室" : "Planning lab"}</summary>
    {open && <div className={styles.experimentsBody}>
      <p>{language === "zh"
        ? "试用工作需求、使用计时和近期节奏估算。自动推算已暂停，可查看已有记录。"
        : "Try work plans, activity tracking and experimental pace estimates. Automatic trials are paused; saved records are available."}</p>
      {children}
    </div>}
  </details>;
}

/** Mounted only after opening the lab in Settings. Uses the existing read stream. */
export function PlanningExperimentContent({ language }: { language: Language }) {
  const [envelope, setEnvelope] = useState(getCapacityStatusSnapshot);
  useEffect(() => {
    let active = true;
    const apply: Parameters<typeof subscribeToCapacityStatus>[0] = (next) => {
      if (active) setEnvelope((current) => current ? acceptNewerEnvelope(current, next) : next);
    };
    const unsubscribe = subscribeToCapacityStatus(apply);
    void getCapacityStatus().then(apply).catch(() => undefined);
    return () => { active = false; unsubscribe(); };
  }, []);
  return <CapacityDemandPanel contextId={envelope?.historyContextId}
    sequence={envelope?.sequence ?? 0} language={language} />;
}
