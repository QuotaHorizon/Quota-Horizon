import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { cancelCodexThreadSearch, continueCodexThreadSearch, getCodexThreadRevision, hasLocalBackend, searchCodexThreads, loadCodexThreadTokens } from "../../api/backend";
import { readSearchBatches } from "./searchBatches";
import { watchThreadChanges } from "./watchThreadChanges";
import type {
  CodexThreadEntry, CodexThreadKind, CodexThreadStatus, CodexThreadTokenTotals, CodexThreadSearchCoverage,
} from "../../types";
import {
  canArchiveThreads, filterThreads, groupThreads, retainVisibleThreadSelection,
} from "./utils";

const SEARCH_DELAY_MS = 300;

export function useThreadList(reportError: (error: unknown) => void) {
  const [threads, setThreads] = useState<CodexThreadEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [query, setQuery] = useState("");
  const [appliedQuery, setAppliedQuery] = useState("");
  const [searchCoverage, setSearchCoverage] = useState<CodexThreadSearchCoverage | null>(null);
  const [searchStale, setSearchStale] = useState(false);
  const [searchStopped, setSearchStopped] = useState(false);
  const [searchClientId] = useState(() => crypto.randomUUID());
  const activeSearchRef = useRef(0);
  const [kind, setKind] = useState<CodexThreadKind | "all">("all");
  const [status, setStatus] = useState<CodexThreadStatus | "all">("active");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [tokens, setTokens] = useState<Record<string, CodexThreadTokenTotals>>({});
  const latestReadRef = useRef(0);
  const inFlightRef = useRef(0);
  const appliedQueryRef = useRef("");
  const initializedRef = useRef(false);
  const threadVersionsRef = useRef(new Map<string, string>());
  const [updatedAt, setUpdatedAt] = useState<number | null>(null);
  const [updateFailed, setUpdateFailed] = useState(false);
  const searchDelayRef = useRef<number | null>(null);

  const refresh = useCallback(async (nextQuery: string, silent = false) => {
    if (!silent) setLoading(true);
    setSearchCoverage(null);
    setSearchStale(false);
    setSearchStopped(false);
    const requestId = latestReadRef.current + 1;
    latestReadRef.current = requestId;
    activeSearchRef.current = requestId;
    inFlightRef.current += 1;
    let firstBatch = true;
    const seenGroups = new Set<string>();
    try {
      return await readSearchBatches({
        start: () => searchCodexThreads(nextQuery, searchClientId, requestId),
        next: (continuation) => continueCodexThreadSearch(searchClientId, requestId, continuation),
        isCurrent: () => requestId === latestReadRef.current,
        onBatch: (response) => {
          const result = response.entries;
          setSearchCoverage(response.coverage);
          const versions = new Map(result.map((item) => [item.sessionId, `${item.updatedAt}:${item.sizeBytes}`]));
          const previousVersions = threadVersionsRef.current;
          setTokens((current) => Object.fromEntries(Object.entries(current).filter(([id]) => versions.has(id) && versions.get(id) === previousVersions.get(id))));
          threadVersionsRef.current = versions;
          setThreads(result);
          const normalizedQuery = nextQuery.trim();
          setAppliedQuery(normalizedQuery);
          if (normalizedQuery) {
            const newGroups = result.map((item) => item.cwd).filter((cwd) => !seenGroups.has(cwd));
            const replaceGroups = firstBatch;
            setExpanded((current) => new Set([...(replaceGroups ? [] : current), ...newGroups]));
            newGroups.forEach((cwd) => seenGroups.add(cwd));
          } else if (normalizedQuery !== appliedQueryRef.current || !initializedRef.current) {
            setExpanded(new Set(result[0] ? [result[0].cwd] : []));
          }
          firstBatch = false;
          initializedRef.current = true;
          appliedQueryRef.current = normalizedQuery;
          setUpdatedAt(Date.now());
          setUpdateFailed(false);
          setSelected((current) => new Set(
            [...current].filter((id) => result.some((item) => item.sessionId === id)),
          ));
        },
      });
    } catch (error) {
      if (requestId === latestReadRef.current) {
        setUpdateFailed(true);
        if (!silent) reportError(error);
      }
      return false;
    } finally {
      inFlightRef.current -= 1;
      if (requestId === latestReadRef.current) setLoading(false);
    }
  }, [reportError, searchClientId]);

  useEffect(() => { void refresh(""); }, []); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => () => {
    latestReadRef.current += 1;
    if (activeSearchRef.current) void cancelCodexThreadSearch(searchClientId, activeSearchRef.current).catch(() => undefined);
    if (searchDelayRef.current !== null) window.clearTimeout(searchDelayRef.current);
  }, [searchClientId]);

  useEffect(() => {
    if (!hasLocalBackend) return;
    const watcher = watchThreadChanges({
      readRevision: getCodexThreadRevision,
      refresh: async () => {
        if (appliedQueryRef.current) { setSearchStale(true); return true; }
        return refresh("", true);
      },
      canRead: () => document.visibilityState !== "hidden" && inFlightRef.current === 0 && searchDelayRef.current == null,
      onError: () => setUpdateFailed(true),
    });
    const resume = () => { if (document.visibilityState !== "hidden") void watcher.check(!appliedQueryRef.current); };
    window.addEventListener("focus", resume);
    document.addEventListener("visibilitychange", resume);
    return () => {
      watcher.dispose();
      window.removeEventListener("focus", resume);
      document.removeEventListener("visibilitychange", resume);
    };
  }, [refresh]);

  const visibleThreads = useMemo(
    () => filterThreads(threads, kind, status),
    [kind, status, threads],
  );
  const visibleSelection = useMemo(
    () => retainVisibleThreadSelection(visibleThreads, selected),
    [selected, visibleThreads],
  );
  useEffect(() => {
    setSelected((current) => retainVisibleThreadSelection(visibleThreads, current));
  }, [visibleThreads]);
  const groups = useMemo(() => groupThreads(visibleThreads), [visibleThreads]);
  const allVisibleSelected = visibleThreads.length > 0
    && visibleThreads.every((item) => visibleSelection.has(item.sessionId));
  const someVisibleSelected = visibleThreads.some((item) => visibleSelection.has(item.sessionId));
  const canArchiveSelected = canArchiveThreads(visibleThreads, visibleSelection);

  const toggleAll = () => setSelected(
    allVisibleSelected ? new Set() : new Set(visibleThreads.map((item) => item.sessionId)),
  );
  const toggleThread = (id: string) => setSelected((current) => {
    const next = new Set(current);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    return next;
  });
  const toggleGroup = async (cwd: string, items: CodexThreadEntry[]) => {
    const next = new Set(expanded);
    if (next.has(cwd)) next.delete(cwd);
    else next.add(cwd);
    setExpanded(next);
    if (expanded.has(cwd)) return;
    const missing = items.map((item) => item.sessionId).filter((id) => !tokens[id]);
    if (!missing.length) return;
    try {
      const totals = await loadCodexThreadTokens(missing);
      const requestedVersions = new Map(items.map((item) => [item.sessionId, `${item.updatedAt}:${item.sizeBytes}`]));
      setTokens((current) => ({
        ...current,
        ...Object.fromEntries(totals.filter((item) => requestedVersions.get(item.sessionId) === threadVersionsRef.current.get(item.sessionId))
          .map((item) => [item.sessionId, item])),
      }));
    } catch {
      // Token details are optional; the session list remains usable if a rollout is incomplete.
    }
  };
  const cancelQueuedSearch = () => {
    if (searchDelayRef.current === null) return;
    window.clearTimeout(searchDelayRef.current);
    searchDelayRef.current = null;
  };
  const queueSearch = (nextQuery: string) => {
    cancelQueuedSearch();
    setLoading(true);
    // Invalidate at the keystroke, not after debounce: the old result must not
    // replace what the user is now asking for. Cancellation is request-scoped.
    latestReadRef.current += 1;
    if (activeSearchRef.current) void cancelCodexThreadSearch(searchClientId, activeSearchRef.current).catch(() => undefined);
    searchDelayRef.current = window.setTimeout(() => {
      searchDelayRef.current = null;
      void refresh(nextQuery);
    }, SEARCH_DELAY_MS);
  };
  const search = () => {
    cancelQueuedSearch();
    void refresh(query);
  };
  const clearSearch = () => {
    cancelQueuedSearch();
    setQuery("");
    void refresh("");
  };
  const stopSearch = () => {
    cancelQueuedSearch();
    latestReadRef.current += 1;
    if (activeSearchRef.current) void cancelCodexThreadSearch(searchClientId, activeSearchRef.current).catch(() => undefined);
    setLoading(false);
    setSearchStopped(true);
  };

  return {
    loading, updatedAt, updateFailed, query, setQuery, appliedQuery, searchCoverage, searchStale, searchStopped, stopSearch, kind, setKind, status, setStatus,
    selected: visibleSelection, setSelected, expanded, tokens,
    visibleThreads, groups, allVisibleSelected, someVisibleSelected, canArchiveSelected,
    refresh, toggleAll, toggleThread,
    toggleGroup, queueSearch, search, clearSearch,
  };
}
