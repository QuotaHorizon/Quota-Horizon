import type { PublicReadReceipt, PublicResetTimeline } from "./types";

interface PublicSourcesApi {
  load(): Promise<PublicResetTimeline>;
  refresh(force: boolean): Promise<PublicResetTimeline>;
  subscribe?(onChange: () => void, onFailure: () => void): () => void;
  markRead?(receipts: PublicReadReceipt[]): Promise<PublicResetTimeline>;
}

export interface PublicSourcesState {
  timeline: PublicResetTimeline | null;
  busy: boolean;
  failed: boolean;
  readBusy?: boolean;
  readFailed?: boolean;
}

/** Shared, single-flight state survives page navigation. Loading or a failed
 * refresh never replaces visible evidence with an empty placeholder. */
export function createPublicSourcesStore(api: PublicSourcesApi) {
  let state: PublicSourcesState = { timeline: null, busy: false, failed: false };
  let pending: Promise<void> | null = null;
  let loading: Promise<void> | null = null;
  let reloadRequested = false;
  let marking: Promise<void> | null = null;
  const queuedReads = new Map<string, number>();
  const activeReads = new Map<string, number>();
  let unsubscribe: (() => void) | undefined;
  const listeners = new Set<() => void>();
  const publish = (patch: Partial<PublicSourcesState>) => {
    state = { ...state, ...patch };
    listeners.forEach((listener) => listener());
  };
  const acceptTimeline = (timeline: PublicResetTimeline) => {
    const checkedAt = (value: PublicResetTimeline) => Math.max(0, ...value.sources.map((source) => Date.parse(source.lastAttemptAt ?? "") || 0));
    if (!state.timeline || (timeline.revisionCount >= state.timeline.revisionCount && checkedAt(timeline) >= checkedAt(state.timeline)
      && (timeline.changes?.readVersion ?? 0) >= (state.timeline.changes?.readVersion ?? 0))) {
      publish({ timeline, failed: false });
    }
  };
  const reload = (afterCurrent = false): Promise<void> => {
    if (loading) { if (afterCurrent) reloadRequested = true; return loading; }
    const operation = (async () => {
      // A native completion event may arrive before the manual invoke resolves.
      // Re-read after it, without issuing a second HTTP collection.
      do {
        reloadRequested = false;
        if (pending) await pending;
        try { acceptTimeline(await api.load()); }
        catch { publish({ failed: true }); }
      } while (reloadRequested);
    })();
    loading = operation.finally(() => { loading = null; });
    return loading;
  };
  const refresh = (force = false): Promise<void> => {
    if (pending) return pending;
    publish({ busy: true, failed: false });
    const operation = (async () => {
      if (!state.timeline) {
        try { acceptTimeline(await api.load()); }
        catch { /* The refresh may recover a transient read failure. */ }
      }
      try { acceptTimeline(await api.refresh(force)); }
      catch { publish({ failed: true }); }
    })();
    pending = operation.finally(() => { pending = null; publish({ busy: false }); });
    return pending;
  };
  const markReceipts = (receipts: PublicReadReceipt[]): Promise<void> => {
    for (const receipt of receipts) {
      if ((activeReads.get(receipt.signalId) ?? 0) >= receipt.revision) continue;
      queuedReads.set(receipt.signalId, Math.max(queuedReads.get(receipt.signalId) ?? 0, receipt.revision));
    }
    if (marking) return marking;
    if (!queuedReads.size) return Promise.resolve();
    publish({ readBusy: true, readFailed: false });
    const operation = Promise.resolve().then(async () => {
      try {
        if (!api.markRead) throw new Error("Reading receipts unavailable");
        while (queuedReads.size) {
          const batch = [...queuedReads].slice(0, 128).map(([signalId, revision]) => ({ signalId, revision }));
          for (const { signalId, revision } of batch) { queuedReads.delete(signalId); activeReads.set(signalId, revision); }
          acceptTimeline(await api.markRead(batch));
          activeReads.clear();
          // Reconcile a concurrent collector without another HTTP collection.
          await reload(true);
        }
      } catch {
        queuedReads.clear(); activeReads.clear();
        publish({ readFailed: true });
      }
    });
    marking = operation.finally(() => { marking = null; publish({ readBusy: false }); });
    return marking;
  };
  const markRead = (shown: PublicResetTimeline) => markReceipts((shown.changes?.items ?? [])
    .filter((item) => item.unread).map(({ current }) => ({ signalId: current.signalId, revision: current.revision })));
  return {
    getSnapshot: () => state,
    subscribe(listener: () => void) {
      listeners.add(listener);
      if (listeners.size === 1) unsubscribe = api.subscribe?.(() => { void reload(true); }, () => publish({ failed: true }));
      return () => {
        listeners.delete(listener);
        if (!listeners.size) { unsubscribe?.(); unsubscribe = undefined; }
      };
    },
    refresh, reload, markRead, markReceipts,
  };
}
