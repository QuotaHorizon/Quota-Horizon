import type { DesktopStatusEnvelope } from "../../../capacity-preview/src/status";

type StatusListener = (status: DesktopStatusEnvelope) => void;
interface CapacityStatusApi {
  read(): Promise<DesktopStatusEnvelope>;
  cached(): Promise<DesktopStatusEnvelope | null>;
  refresh(): Promise<DesktopStatusEnvelope>;
  listen(listener: StatusListener): Promise<() => void>;
}

/** One ordered snapshot per webview, not per page. The native subscription
 * remains attached between pages so account/reader invalidations are not lost.
 * Nothing is persisted in browser storage; native code owns account identity. */
export function createCapacityStatusStore(api: CapacityStatusApi) {
  let snapshot: DesktopStatusEnvelope | null = null;
  let subscription: Promise<void> | null = null;
  let reading: Promise<DesktopStatusEnvelope> | null = null;
  let refreshing: Promise<DesktopStatusEnvelope> | null = null;
  const listeners = new Set<StatusListener>();
  const accept = (next: DesktopStatusEnvelope) => {
    if (snapshot && next.sequence < snapshot.sequence) return snapshot;
    if (snapshot !== next) {
      snapshot = next;
      listeners.forEach((listener) => listener(next));
    }
    return snapshot;
  };
  const start = () => {
    if (!subscription) {
      subscription = api.listen(accept).then(() => undefined).catch(() => {
        // A failed listener must not break direct reads, and can be retried.
        subscription = null;
      });
    }
    return subscription;
  };
  return {
    getSnapshot: () => snapshot,
    accept,
    subscribe(listener: StatusListener) {
      listeners.add(listener);
      void start();
      return () => { listeners.delete(listener); };
    },
    async cached() {
      await start();
      const value = await api.cached();
      return value ? accept(value) : snapshot;
    },
    read(): Promise<DesktopStatusEnvelope> {
      if (!reading) reading = (async () => {
        // Register before reading: an account event during the request must
        // outrank that older response, including when no page is mounted.
        await start();
        return accept(await api.read());
      })().finally(() => { reading = null; });
      return reading;
    },
    refresh(): Promise<DesktopStatusEnvelope> {
      if (!refreshing) refreshing = (async () => {
        await start();
        return accept(await api.refresh());
      })().finally(() => { refreshing = null; });
      return refreshing;
    },
  };
}
