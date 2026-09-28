import type {DesktopPaceTrialsEnvelope} from "../../../../capacity-preview/src/status";

export function createPaceTrialStore(context: string, read: (context: string, offset: number) => Promise<DesktopPaceTrialsEnvelope>) {
  let snapshot = {value: null as DesktopPaceTrialsEnvelope | null, open: false, offset: 0, loading: false, failed: false};
  const listeners = new Set<() => void>();
  let pending: Promise<void> | null = null, again = false;
  const publish = (next: typeof snapshot) => {snapshot = next; listeners.forEach(fn => fn());};
  function reload(): Promise<void> {
    if (!snapshot.open) return Promise.resolve();
    if (pending) {again = true; return pending;}
    publish({...snapshot, loading: true});
    pending = (async () => {
      do {
        again = false;
        const offset = snapshot.offset;
        try {
          const value = await read(context, offset);
          if (!snapshot.open || offset !== snapshot.offset) continue;
          if (value.historyContextId !== context || value.offset !== offset) publish({...snapshot, value: null, failed: true});
          else if (value.status === "failed") publish({...snapshot, failed: true});
          else publish({...snapshot, value, failed: false});
        } catch {if (snapshot.open && offset === snapshot.offset) publish({...snapshot, failed: true});}
      } while (again && snapshot.open && listeners.size > 0);
    })().finally(() => {pending = null; publish({...snapshot, loading: false});});
    return pending;
  }
  return {
    getSnapshot: () => snapshot, reload,
    subscribe(fn: () => void) {listeners.add(fn); return () => {listeners.delete(fn);};},
    setOpen(open: boolean) {publish({...snapshot, open}); if (open) void reload();},
    setOffset(offset: number) {if (offset < 0 || offset > 100_000 || !Number.isInteger(offset)) return; publish({...snapshot, offset, value: null, failed: false}); void reload();},
  };
}
