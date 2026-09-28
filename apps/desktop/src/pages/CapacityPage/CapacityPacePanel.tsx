import {useEffect, useMemo, useSyncExternalStore} from "react";
import {getCapacityPace, getCapacityPaceTrials} from "../../api/backend";
import type {Language} from "../../i18n";
import {createPaceStore} from "./paceStore";
import {CapacityPaceView} from "./CapacityPaceView";
import {PaceTrialHistory} from "./PaceTrialHistory";

export function CapacityPacePanel({contextId, sequence, language}: {contextId: string; sequence: number; language: Language}) {
  const store = useMemo(() => createPaceStore(contextId, getCapacityPace), [contextId]);
  const snapshot = useSyncExternalStore(store.subscribe, store.getSnapshot, store.getSnapshot);
  useEffect(() => {void store.reload();}, [store, sequence]);
  useEffect(() => {
    const reload = () => {if (document.visibilityState === "visible") void store.reload();};
    window.addEventListener("focus", reload); document.addEventListener("visibilitychange", reload);
    const timer = window.setInterval(reload, 30_000);
    return () => {window.clearInterval(timer); window.removeEventListener("focus", reload); document.removeEventListener("visibilitychange", reload);};
  }, [store]);
  return <><CapacityPaceView {...snapshot} contextId={contextId} language={language} onRetry={() => void store.reload()}/><PaceTrialHistory key={contextId} contextId={contextId} sequence={sequence} language={language} read={getCapacityPaceTrials}/></>;
}
