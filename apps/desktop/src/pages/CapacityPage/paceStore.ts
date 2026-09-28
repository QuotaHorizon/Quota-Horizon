import type {DesktopPaceEnvelope} from "../../../../capacity-preview/src/status";

export function createPaceStore(context: string, read: (context: string) => Promise<DesktopPaceEnvelope>) {
  let snapshot = {value: null as DesktopPaceEnvelope | null, failed: false, loading: false};
  const listeners = new Set<() => void>();
  let loading: Promise<void> | null = null, again = false;
  const publish = (next: typeof snapshot) => {snapshot = next; listeners.forEach(fn => fn());};
  function reload(): Promise<void> {
    if (loading) {again = true; return loading;}
    publish({...snapshot, loading: true});
    loading = (async () => {
      do {
        again = false;
        try {
          const next = await read(context);
          if (next.historyContextId !== context) publish({...snapshot, value: null, failed: true});
          else if (next.status === "failed") publish({...snapshot, failed: true});
          else publish({...snapshot, value: next, failed: false});
        } catch {publish({...snapshot, failed: true});}
      } while (again && listeners.size > 0);
    })().finally(() => {loading = null; publish({...snapshot, loading: false});});
    return loading;
  }
  return {getSnapshot: () => snapshot, reload, subscribe(fn: () => void) {listeners.add(fn); return () => {listeners.delete(fn);};}};
}
