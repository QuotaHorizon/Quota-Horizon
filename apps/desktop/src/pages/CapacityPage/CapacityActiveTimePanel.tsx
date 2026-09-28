import { useEffect, useMemo, useSyncExternalStore } from "react";
import { getCapacityActiveTime, updateCapacityActiveTime, subscribeToCapacityActiveTime } from "../../api/backend";
import type { Language } from "../../i18n";
import { createActiveTimeStore } from "./activeTimeStore";
import { CapacityActiveTimeView } from "./CapacityActiveTimeView";
import { ActivityQuotaPanel } from "./ActivityQuotaPanel";

export function CapacityActiveTimePanel({contextId,sequence,language,compact,onOpen}:{contextId:string;sequence:number;language:Language;compact?:boolean;onOpen?:()=>void}) {
  const store=useMemo(()=>createActiveTimeStore(contextId,{read:getCapacityActiveTime,update:updateCapacityActiveTime,subscribe:subscribeToCapacityActiveTime}),[contextId]);
  const snapshot=useSyncExternalStore(store.subscribe,store.getSnapshot,store.getSnapshot);
  useEffect(()=>{void store.reload();},[store,sequence]);
  useEffect(()=>{
    const reload=()=>{if (document.visibilityState==="visible") void store.reload();};
    window.addEventListener("focus",reload);document.addEventListener("visibilitychange",reload);
    const timer=window.setInterval(reload,30_000);
    return ()=>{window.clearInterval(timer);window.removeEventListener("focus",reload);document.removeEventListener("visibilitychange",reload);};
  },[store]);
  return <CapacityActiveTimeView key={contextId} {...snapshot} contextId={contextId} language={language} compact={compact} onOpen={onOpen} onSave={store.save} onRetry={()=>void store.reload()} renderEvidence={(observationId,revision)=><ActivityQuotaPanel key={observationId} contextId={contextId} observationId={observationId} language={language} sequence={sequence} recordRevision={revision}/>}/>;
}
