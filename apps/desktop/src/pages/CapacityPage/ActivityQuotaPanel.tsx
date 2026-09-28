import {useEffect,useMemo,useSyncExternalStore} from "react";
import {getCapacityActiveTimeQuota} from "../../api/backend";
import type {Language} from "../../i18n";
import {createActivityQuotaStore} from "./activityQuotaStore";
import {ActivityQuotaView} from "./ActivityQuotaView";

export function ActivityQuotaPanel({contextId,observationId,language,sequence,recordRevision}:{contextId:string;observationId:string;language:Language;sequence:number;recordRevision:number}) {
  const store=useMemo(()=>createActivityQuotaStore(contextId,observationId,getCapacityActiveTimeQuota),[contextId,observationId]);
  const snapshot=useSyncExternalStore(store.subscribe,store.getSnapshot,store.getSnapshot);
  useEffect(()=>{void store.reload();},[store,sequence,recordRevision]);
  useEffect(()=>{const refresh=()=>{if(document.visibilityState==="visible") void store.reload();};window.addEventListener("focus",refresh);document.addEventListener("visibilitychange",refresh);return ()=>{window.removeEventListener("focus",refresh);document.removeEventListener("visibilitychange",refresh);};},[store]);
  return <ActivityQuotaView {...snapshot} language={language} onRetry={()=>void store.reload()}/>;
}
