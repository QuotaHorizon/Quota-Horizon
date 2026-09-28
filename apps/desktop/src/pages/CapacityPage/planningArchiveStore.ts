import type {DesktopPlanningArchiveEnvelope, PlanningArchiveQuery} from "../../../../capacity-preview/src/status";

export type PlanningArchiveReader = (context: string, query: PlanningArchiveQuery) => Promise<DesktopPlanningArchiveEnvelope>;
export const archiveQueryKey = (q: PlanningArchiveQuery) => q.kind === "list" ? `list:${q.offset}`
  : `${q.kind}:${q.source.kind}:${q.source.id}:${q.kind === "quota" ? q.observationId : q.offset}`;

export function createPlanningArchiveStore(context: string, query: PlanningArchiveQuery, read: PlanningArchiveReader) {
  let snapshot = {value: null as DesktopPlanningArchiveEnvelope | null, failed: false, loading: false};
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
          const next = await read(context, query);
          if (next.historyContextId !== context || archiveQueryKey(next.query) !== archiveQueryKey(query)
            || (next.result && next.result.kind !== query.kind)) publish({...snapshot, value: null, failed: true});
          else if (next.status === "failed") publish({...snapshot, failed: true});
          else publish({...snapshot, value: next, failed: false});
        } catch {publish({...snapshot, failed: true});}
      } while (again && listeners.size > 0);
    })().finally(() => {loading = null; publish({...snapshot, loading: false});});
    return loading;
  }
  return {getSnapshot: () => snapshot, reload, subscribe(fn: () => void) {listeners.add(fn); return () => {listeners.delete(fn);};}};
}
