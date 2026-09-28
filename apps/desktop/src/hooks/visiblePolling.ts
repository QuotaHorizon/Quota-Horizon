/** UI-only polling. Background quota monitoring has a separate native owner. */
export function createVisiblePoller<T>({ read, onValue, onError, onBusy }: {
  read: () => Promise<T>;
  onValue: (value: T) => void;
  onError: (error: unknown) => void;
  onBusy?: (busy: boolean) => void;
}) {
  let active = false;
  let disposed = false;
  let running = false;
  let queued = false;
  let revision = 0;
  let key: string | null = null;
  let interval = 60_000;
  let timer: ReturnType<typeof setTimeout> | null = null;

  const clearTimer = () => {
    if (timer !== null) clearTimeout(timer);
    timer = null;
  };
  const refresh = async () => {
    if (disposed || !active) return;
    clearTimer();
    if (running) {
      queued = true;
      return;
    }
    const requestRevision = revision;
    running = true;
    queued = false;
    onBusy?.(true);
    try {
      const value = await read();
      if (!disposed && active && revision === requestRevision) onValue(value);
    } catch (error) {
      if (!disposed && active && revision === requestRevision) onError(error);
    } finally {
      running = false;
      if (!disposed) onBusy?.(false);
      if (!disposed && active) {
        if (queued || revision !== requestRevision) void refresh();
        else timer = setTimeout(() => void refresh(), interval);
      }
    }
  };

  return {
    refresh,
    configure(next: { active: boolean; key: string; intervalMs: number }) {
      if (disposed) return;
      const changed = key !== next.key || active !== next.active;
      const nextInterval = Number.isFinite(next.intervalMs)
        ? Math.min(86_400_000, Math.max(1_000, next.intervalMs)) : 60_000;
      const intervalChanged = interval !== nextInterval;
      interval = nextInterval;
      key = next.key;
      active = next.active;
      if (changed) revision += 1;
      if (!active) {
        clearTimer();
        queued = false;
      } else if (changed) {
        void refresh();
      } else if (intervalChanged && !running) {
        clearTimer();
        timer = setTimeout(() => void refresh(), interval);
      }
    },
    dispose() {
      disposed = true;
      active = false;
      queued = false;
      revision += 1;
      clearTimer();
    },
  };
}
