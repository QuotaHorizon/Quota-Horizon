import { useCallback, useEffect, useLayoutEffect, useRef } from "react";
import { createVisiblePoller } from "./visiblePolling";

const useCommitEffect = typeof window === "undefined" ? useEffect : useLayoutEffect;

export function useVisiblePolling<T>(options: {
  active: boolean;
  queryKey: string;
  intervalMs: number;
  read: () => Promise<T>;
  onValue: (value: T) => void;
  onError: (error: unknown) => void;
  onBusy?: (busy: boolean) => void;
}) {
  const latest = useRef(options);
  const poller = useRef<ReturnType<typeof createVisiblePoller<T>> | null>(null);

  useCommitEffect(() => {
    const current = createVisiblePoller<T>({
      read: () => latest.current.read(),
      onValue: (value) => latest.current.onValue(value),
      onError: (error) => latest.current.onError(error),
      onBusy: (busy) => latest.current.onBusy?.(busy),
    });
    poller.current = current;
    return () => { current.dispose(); poller.current = null; };
  }, []);

  useCommitEffect(() => {
    // Commit callbacks and query identity together; an abandoned render must
    // not redirect an in-flight read or relabel its results.
    latest.current = options;
    poller.current?.configure({ active: options.active, key: options.queryKey, intervalMs: options.intervalMs });
  });

  return useCallback(() => { void poller.current?.refresh(); }, []);
}
