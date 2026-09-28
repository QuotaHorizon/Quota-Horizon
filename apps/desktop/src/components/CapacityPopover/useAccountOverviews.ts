import { useCallback, useEffect, useRef, useState } from "react";
import { getCapacityAccountOverviews, hasLocalBackend, refreshAccountUsage, subscribeToBackendEvents, subscribeToCapacityStatus } from "../../api/backend";
import { isFreshUsageRefresh, runBoundedAccountRefresh, type AccountRefreshProgress, type AccountRefreshResult } from "../../hooks/accountRefreshCoordinator";
import { createAccountRefreshPolicy, createOverviewLoader, type AccountOverview } from "./accountOverview";

export function useAccountOverviews(visible: boolean, preview: AccountOverview[]) {
  const [accounts, setAccounts] = useState<AccountOverview[]>(hasLocalBackend ? [] : preview);
  const [privacyMode, setPrivacyMode] = useState(true);
  const [loading, setLoading] = useState(hasLocalBackend);
  const [refreshing, setRefreshing] = useState(false);
  const [loadFailed, setLoadFailed] = useState(false);
  const [progress, setProgress] = useState<AccountRefreshProgress | null>(null);
  const [result, setResult] = useState<AccountRefreshResult | null>(null);
  const accountsRef = useRef(accounts);
  const visibleRef = useRef(visible);
  visibleRef.current = visible;
  const active = useRef(false);
  const policy = useRef(createAccountRefreshPolicy());
  const loader = useRef<ReturnType<typeof createOverviewLoader> | null>(null);
  const inFlight = useRef<Promise<void> | null>(null);

  const refreshIds = useCallback((ids: string[]): Promise<void> => {
    if (!hasLocalBackend || !active.current || !ids.length) return Promise.resolve();
    if (inFlight.current) return inFlight.current;
    policy.current.started(ids, Date.now());
    setRefreshing(true);
    const request = runBoundedAccountRefresh(ids, async (id) => {
      const startedAt = Date.now();
      const usage = await refreshAccountUsage(id);
      if (!isFreshUsageRefresh(usage, startedAt)) throw new Error(usage.error || "Cached usage retained");
    }, { onProgress: (next) => { if (active.current) setProgress(next); } }).then((outcome) => {
      for (const id of outcome.succeededIds) policy.current.completed(id, true, Date.now());
      for (const failure of outcome.failures) policy.current.completed(failure.id, false, Date.now());
      if (active.current) setResult(outcome);
    }).finally(() => {
      inFlight.current = null;
      if (!active.current) return;
      setRefreshing(false);
      setProgress(null);
      // Success and failure both re-read authoritative rows, with plans from
      // the same cache. Never merge a late response back into a deleted row.
      void loader.current?.reload();
    });
    inFlight.current = request;
    return request;
  }, []);

  const reload = useCallback(() => loader.current?.reload() ?? Promise.resolve(), []);
  const refreshAll = useCallback(() => refreshIds(accountsRef.current.map(({ id }) => id)), [refreshIds]);

  useEffect(() => {
    active.current = true;
    if (!hasLocalBackend) return () => { active.current = false; };
    const reader = createOverviewLoader(getCapacityAccountOverviews, (snapshot) => {
      accountsRef.current = snapshot.accounts;
      setAccounts(snapshot.accounts);
      setPrivacyMode(snapshot.privacyMode);
      setLoading(false);
      setLoadFailed(false);
      if (!inFlight.current) void refreshIds(policy.current.due(snapshot.accounts, Date.now(), visibleRef.current));
    }, () => { setLoading(false); setLoadFailed(true); });
    loader.current = reader;
    const unsubscribe = subscribeToBackendEvents(() => void reader.reload(), (status) => {
      if (status.ok) {
        policy.current.reset();
        void reader.reload();
      }
    }, () => void reader.reload());
    const unsubscribeStatus = subscribeToCapacityStatus(() => void reader.reload());
    void reader.reload();
    return () => {
      active.current = false;
      reader.dispose();
      if (loader.current === reader) loader.current = null;
      unsubscribe();
      unsubscribeStatus();
    };
  }, [refreshIds]);

  useEffect(() => {
    if (!hasLocalBackend || !visible) return;
    void reload();
    const timer = window.setInterval(() => void reload(), 30_000);
    return () => window.clearInterval(timer);
  }, [reload, visible]);

  return { accounts, privacyMode, busy: loading || refreshing, loadFailed, progress, result, reload, refreshAll };
}
