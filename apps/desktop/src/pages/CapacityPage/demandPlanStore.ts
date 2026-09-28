import type { DesktopDemandPlanEnvelope, DesktopDemandPlanUpdate } from "../../../../capacity-preview/src/status";
import { acceptDemandPlan } from "./demandPlanModel";

export interface DemandPlanApi {
  read: (context: string) => Promise<DesktopDemandPlanEnvelope>;
  update: (request: DesktopDemandPlanUpdate) => Promise<DesktopDemandPlanEnvelope>;
  subscribe?: (changed: () => void) => () => void;
}

export function createDemandPlanStore(context: string, api: DemandPlanApi) {
  let snapshot = { value: null as DesktopDemandPlanEnvelope | null, failed: false, busy: false };
  const listeners = new Set<() => void>();
  let unsubscribe: (() => void) | undefined;
  let loading: Promise<void> | null = null;
  let again = false;
  let mutationEpoch = 0;
  const publish = (next: typeof snapshot) => { snapshot = next; listeners.forEach((listener) => listener()); };
  function reload(): Promise<void> {
    if (loading) { again = true; return loading; }
    loading = (async () => {
      do {
        again = false;
        const epoch = mutationEpoch;
        try {
          const next = await api.read(context);
          if (epoch === mutationEpoch) publish({ ...snapshot, value: acceptDemandPlan(snapshot.value, next, context), failed: next.status === "failed" || next.historyContextId !== context });
        } catch { if (epoch === mutationEpoch) publish({ ...snapshot, failed: true }); }
      } while (again);
    })().finally(() => { loading = null; });
    return loading;
  }
  async function save(request: DesktopDemandPlanUpdate): Promise<boolean> {
    if (snapshot.busy || request.historyContextId !== context) return false;
    ++mutationEpoch; // Pending pre-save reads cannot overwrite the acknowledgment.
    publish({ ...snapshot, busy: true });
    let saved = false;
    try {
      const next = await api.update(request);
      publish({ ...snapshot, value: acceptDemandPlan(snapshot.value, next, context), failed: next.historyContextId !== context });
      saved = next.historyContextId === context && next.status === "updated";
    } catch {
      if (snapshot.value) publish({ ...snapshot, value: { ...snapshot.value, status: "failed", reasonCode: "plan_save_failed", allowances: [] } });
    } finally { publish({ ...snapshot, busy: false }); }
    // Only reload after success: conflicts/errors must remain actionable, with
    // the editor still referring to the revision the user actually saw.
    if (saved) void reload();
    return saved;
  }
  return {
    getSnapshot: () => snapshot,
    reload, save,
    subscribe(listener: () => void) {
      listeners.add(listener);
      if (listeners.size === 1) unsubscribe = api.subscribe?.(() => { void reload(); });
      return () => { listeners.delete(listener); if (!listeners.size) { unsubscribe?.(); unsubscribe = undefined; } };
    },
  };
}
