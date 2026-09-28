import type { DesktopActiveTimeEnvelope, DesktopActiveTimeUpdate } from "../../../../capacity-preview/src/status";
import { acceptActiveTime } from "./activeTimeModel";

export interface ActiveTimeApi {
  read: (context: string) => Promise<DesktopActiveTimeEnvelope>;
  update: (request: DesktopActiveTimeUpdate) => Promise<DesktopActiveTimeEnvelope>;
  subscribe?: (changed: () => void) => () => void;
}
export function createActiveTimeStore(context: string, api: ActiveTimeApi) {
  let snapshot = { value: null as DesktopActiveTimeEnvelope | null, failed: false, busy: false };
  const listeners = new Set<() => void>();
  let unsubscribe: (() => void) | undefined;
  let loading: Promise<void> | null = null;
  let again = false; let mutationEpoch = 0;
  const publish = (next: typeof snapshot) => { snapshot=next; listeners.forEach((listener)=>listener()); };
  function reload(): Promise<void> {
    if (loading) { again=true; return loading; }
    loading=(async()=>{
      do {
        again=false; const epoch=mutationEpoch;
        try {
          const next=await api.read(context);
          if (epoch===mutationEpoch) publish({...snapshot,value:acceptActiveTime(snapshot.value,next,context),failed:next.status==="failed" || next.historyContextId!==context});
        } catch { if (epoch===mutationEpoch) publish({...snapshot,failed:true}); }
      } while (again);
    })().finally(()=>{loading=null;});
    return loading;
  }
  async function save(request: DesktopActiveTimeUpdate): Promise<boolean> {
    if (snapshot.busy || request.historyContextId!==context) return false;
    ++mutationEpoch; publish({...snapshot,busy:true}); let saved=false;
    try {
      const next=await api.update(request);
      publish({...snapshot,value:acceptActiveTime(snapshot.value,next,context),failed:next.historyContextId!==context});
      saved=next.historyContextId===context && next.status==="updated";
    } catch {
      publish({...snapshot,failed:!snapshot.value,value:snapshot.value ? {...snapshot.value,status:"failed",reasonCode:"activity_save_failed"} : null});
    } finally {publish({...snapshot,busy:false});}
    if (saved) void reload();
    return saved;
  }
  return { getSnapshot:()=>snapshot,reload,save,subscribe(listener:()=>void) {
    listeners.add(listener);
    if (listeners.size===1) unsubscribe=api.subscribe?.(()=>{void reload();});
    return ()=>{listeners.delete(listener);if (!listeners.size) {unsubscribe?.();unsubscribe=undefined;}};
  } };
}
