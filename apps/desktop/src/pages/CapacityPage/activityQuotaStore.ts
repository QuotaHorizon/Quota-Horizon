import type { DesktopActiveTimeQuotaEnvelope } from "../../../../capacity-preview/src/status";

export type ActivityQuotaRead = (context: string, observation: string) => Promise<DesktopActiveTimeQuotaEnvelope>;
export function createActivityQuotaStore(context: string, observation: string, read: ActivityQuotaRead) {
  let snapshot = {value: null as DesktopActiveTimeQuotaEnvelope | null, failed: false, loading: false};
  const listeners = new Set<() => void>();
  let pending: Promise<void> | null = null; let again = false;
  const publish = (next: typeof snapshot) => {snapshot=next;listeners.forEach(listener=>listener());};
  function reload(): Promise<void> {
    if (pending) {again=true;return pending;}
    publish({...snapshot,loading:true});
    pending=(async()=>{
      do {
        again=false;
        try {
          const value=await read(context,observation);
          if (value.historyContextId!==context || value.observationId!==observation) publish({...snapshot,value:null,failed:true});
          else if (value.status==="failed") publish({...snapshot,failed:true});
          else publish({...snapshot,value,failed:false});
        } catch {publish({...snapshot,failed:true});}
      } while (again);
    })().finally(()=>{pending=null;publish({...snapshot,loading:false});});
    return pending;
  }
  return {getSnapshot:()=>snapshot,reload,subscribe(listener:()=>void) {listeners.add(listener);return ()=>{listeners.delete(listener);};}};
}
