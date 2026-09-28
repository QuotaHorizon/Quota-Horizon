import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore, type CSSProperties } from "react";
import {
  ArrowUpRight,
  Briefcase,
  Clock3,
  Gauge,
  Pause,
  Plus,
  RefreshCw,
  Settings2,
  ShieldCheck,
  Trash2,
  TriangleAlert,
  UsersRound,
} from "lucide-react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  getCapacityStatus,
  getCapacityStatusSnapshot,
  getCapacityCachedStatus,
  getCapacityWorkPlan,
  hasLocalBackend,
  refreshCapacityStatus,
  requestAccountSwitchFromPopover,
  showDashboardFromBubble,
  showSettingsFromPopover,
  showResetIntelligenceFromPopover,
  subscribeToBackendEvents,
  subscribeToCapacityStatus,
  updateCapacityWorkSchedule,
} from "../../api/backend";
import { useLanguage } from "../../hooks/useLanguage";
import { useAccountOverviews } from "./useAccountOverviews";
import { AccountOverviewCard } from "./AccountOverviewCard";
import { ShortQuotaCard } from "./ShortQuotaCard";
import { ResetNoticeChip } from "./ResetNoticeChip";
import { publicSourcesRuntime } from "../../pages/ResetIntelligencePage/publicSourcesRuntime";
import { upcomingResetNotices } from "../../pages/ResetIntelligencePage/upcomingResetModel";
import { unreadResetProgress } from "../../pages/ResetIntelligencePage/resetChangesModel";
import { planHasNoShortQuotaWindow } from "../../utils/accountUsageWindows";
import { capacityQuotaNotice, capacityRefreshFeedback, capacityIssueMessage } from "../../pages/CapacityPage/statusCopy";
import type { AccountOverview } from "./accountOverview";
import type {
  DesktopStatusEnvelope,
  DesktopWorkPlanEnvelope,
  DesktopWorkSchedulePeriod,
} from "../../../../capacity-preview/src/status";
import {
  acceptNewerEnvelope,
  capacityTone,
  durationLabel,
  idleCapacityEnvelope,
  localResetTime,
  minuteSpanLabel,
  minuteToTimeValue,
  paceTone,
  percentLabel,
  percentValueLabel,
  planPercentLabel,
  popoverWindows,
  resetCountdown,
  targetTimeLabel,
  timeValueToMinute,
  updatedLabel,
} from "./presentation";
import styles from "./index.module.less";

type PopoverPage = "capacity" | "plan" | "accounts";

const COPY = {
  en: {
    official: "OFFICIAL CAPACITY",
    live: "Live",
    stale: "Stale",
    unavailable: "Unavailable",
    partial: "Setup needed",
    capacity: "Capacity",
    plan: "Work Plan",
    accounts: "Accounts",
    remaining: "remaining",
    resets: "Resets",
    account: "Current plan",
    source: "Official Codex",
    dashboard: "Dashboard",
    settings: "Settings",
    refresh: "Refresh capacity",
    retry: "Retry",
    waiting: "Reading your Codex capacity…",
    noSnapshot: "No capacity snapshot is available yet.",
    select: "Choose the Codex installation to trust in the dashboard.",
    open: "Open dashboard",
    planTitle: "Daily schedule",
    planSubtitle: "Spend only inside your usable hours",
    enabled: "Plan on",
    disabled: "Plan off",
    actual: "Actual",
    target: "Stop target",
    working: "Working now",
    off: "Off now",
    nextStop: "Next stop",
    frozenAt: "Frozen at",
    dailyBudget: "Daily budget",
    usable: "usable / day",
    onPace: "On pace",
    withinGuard: "Within 5% guard",
    overGuard: "Over plan",
    ahead: "ahead",
    overBy: "over by",
    offPeriods: "Off periods",
    addPeriod: "Add off period",
    allDay: "No off periods · all day usable",
    baselineNote: "Quota allocated across your available hours",
    planLiveSource: "Official quota",
    planSnapshotSource: "Official quota snapshot",
    planCachedSource: "Saved current-account quota",
    planUnavailable: "A weekly quota window is required to calculate pace.",
    scheduleUnavailable: "Work Plan storage is unavailable.",
    synced: "Schedule saved",
    conflict: "Schedule changed elsewhere · latest version loaded",
    saveFailed: "Could not save schedule",
    savedAccounts: "Saved account quota",
    current: "Current",
    accountsEmpty: "No saved accounts yet.",
    refreshAccounts: "Refresh all",
    accountRefreshCached: "Cached usage retained",
    switchAccount: "Review safe switch",
  },
  zh: {
    official: "官方额度",
    live: "实时",
    stale: "旧快照",
    unavailable: "暂不可用",
    partial: "需要设置",
    capacity: "额度",
    plan: "工作计划",
    accounts: "账号",
    remaining: "剩余",
    resets: "重置",
    account: "当前计划",
    source: "官方 Codex",
    dashboard: "仪表板",
    settings: "设置",
    refresh: "刷新额度",
    retry: "重试",
    waiting: "正在读取 Codex 额度…",
    noSnapshot: "尚未获得可用的额度快照。",
    select: "请在仪表板选择 Codex 安装。",
    open: "打开仪表板",
    planTitle: "每日安排",
    planSubtitle: "只在你的可用时段内分配额度",
    enabled: "计划已开启",
    disabled: "计划未开启",
    actual: "实际剩余",
    target: "停用前目标",
    working: "当前可工作",
    off: "当前停用",
    nextStop: "下次停用",
    frozenAt: "目标冻结于",
    dailyBudget: "每日预算",
    usable: "每日可用",
    onPace: "进度健康",
    withinGuard: "处于 5% 缓冲区",
    overGuard: "已超计划",
    ahead: "领先",
    overBy: "超出",
    offPeriods: "停用时段",
    addPeriod: "添加停用时段",
    allDay: "没有停用时段 · 全天可用",
    baselineNote: "按可用时段分配剩余额度",
    planLiveSource: "官方实时额度",
    planSnapshotSource: "官方额度快照",
    planCachedSource: "当前账号已保存额度",
    planUnavailable: "需要有效的周额度窗口才能计算计划进度。",
    scheduleUnavailable: "Work Plan 本地存储暂不可用。",
    synced: "日程已保存",
    conflict: "日程已在别处变化 · 已载入最新版本",
    saveFailed: "日程保存失败",
    savedAccounts: "已保存账号额度",
    current: "当前",
    accountsEmpty: "尚无已保存账号。",
    refreshAccounts: "全部刷新",
    accountRefreshCached: "已保留缓存额度",
    switchAccount: "预览安全切换",
  },
} as const;

function previewEnvelope(mode: string | null): DesktopStatusEnvelope {
  if (hasLocalBackend) return idleCapacityEnvelope();
  const now = Date.now();
  if (mode === "error") {
    return {
      schemaVersion: "1.0", sequence: 1, lifecycle: "error", status: null, candidates: [],
      selectedExecutableId: null,
      issue: { code: "app_server_timeout", message: "The Codex capacity service did not respond in time.", retryAfterMs: null },
      persistenceEnabled: false,
    };
  }
  if (mode === "selection") {
    return {
      schemaVersion: "1.0", sequence: 1, lifecycle: "selection_required", status: null,
      candidates: [], selectedExecutableId: null, issue: null, persistenceEnabled: false,
    };
  }
  if (mode !== "ready" && mode !== "stale" && mode !== "cached") return idleCapacityEnvelope();
  const cached = mode === "cached";
  const stale = mode === "stale" || cached;
  return {
    schemaVersion: "1.0",
    sequence: 1,
    lifecycle: stale ? "stale" : "ready",
    status: {
      schemaVersion: "1.0",
      capturedAt: new Date(now - (stale ? 42 * 60_000 : 25_000)).toISOString(),
      codexVersion: "0.116.0",
      account: { authMode: "chatgpt", planType: "Pro", bindingStatus: "stable" },
      dataStatus: {
        availability: stale ? "partial" : "complete",
        freshness: stale ? "stale" : "live",
        compatibility: "tested",
        reasonCodes: cached ? ["managed_quota_only"] : stale ? ["app_server_timeout"] : [],
      },
      ...(cached ? { quotaSource: "managed_account_cache" as const, quotaFreshness: "stale" as const } : {}),
      quotaWindows: [
        { limitId: "codex:short", label: "5h", windowMinutes: 300, usedPercent: 6, remainingPercent: 94, resetsAt: new Date(now + 2.4 * 3_600_000).toISOString() },
        { limitId: "codex:primary", label: "1w", windowMinutes: 10_080, usedPercent: 28, remainingPercent: 72, resetsAt: new Date(now + 4.2 * 86_400_000).toISOString() },
      ],
      resetCredits: { summaryStatus: "available", availableCount: 0, detailsStatus: "complete" },
      usage: { availability: "complete", hasSummary: true, reasonCodes: [] },
      diagnosticCodes: [],
    },
    candidates: [],
    selectedExecutableId: "bundled-codex",
    issue: stale && !cached ? { code: "app_server_timeout", message: "Showing the last trusted snapshot.", retryAfterMs: null } : null,
    persistenceEnabled: false,
  };
}

function previewWorkPlan(mode: string | null): DesktopWorkPlanEnvelope {
  const now = Date.now();
  return {
    schemaVersion: "1.0",
    status: mode === "error" ? "failed" : "available",
    reasonCode: mode === "error" ? "work_plan_window_unavailable" : "work_plan_available",
    schedule: mode === "error" ? null : {
      revision: 3, enabled: true,
      offPeriods: [{ startMinuteOfDay: 120, endMinuteOfDay: 600 }],
      updatedAt: new Date(now - 60_000).toISOString(),
    },
    baseline: mode === "error" ? null : {
      kind: "schedule_baseline",
      limitId: "codex:primary",
      source: "app_server_status",
      observedAt: new Date(now - 25_000).toISOString(),
      freshness: "live",
      comparison: {
        enabled: true,
        segment: "working",
        targetAt: new Date(now + 7 * 3_600_000).toISOString(),
        expectedRemainingPercent: 61,
        actualRemainingPercent: 72,
        overspendPercent: 0,
        state: "on_pace",
        dailyBudgetPercent: 100 / 7,
        usableMinutesPerDay: 960,
        offMinutesPerDay: 480,
        windowExpired: false,
      },
    },
  };
}

// Browser-only visual fixtures. Native windows always load saved-account data.
function previewAccounts(mode: string | null): AccountOverview[] {
  if (hasLocalBackend || !mode) return [];
  const now = Date.now();
  return [28, 81].map((remaining, index) => {
    const workPlan = previewWorkPlan("ready");
    if (workPlan.baseline) {
      workPlan.baseline.source = "managed_account_cache";
      workPlan.baseline.freshness = "stale";
      workPlan.baseline.comparison.actualRemainingPercent = remaining;
      workPlan.baseline.comparison.expectedRemainingPercent = index ? 75.6 : 26.4;
    }
    return {
      id: `preview-${index}`, email: `demo${index + 1}@example.com`, note: "",
      plan: index ? "Plus" : "Pro", active: !index, workPlan,
      usage: {
        primary: index ? { usedPercent: 16, remainingPercent: 84, windowMinutes: 300, resetsAt: Math.floor(now / 1000) + 7200 } : null,
        secondary: { usedPercent: 100 - remaining, remainingPercent: remaining, windowMinutes: 10080, resetsAt: Math.floor(now / 1000) + (index ? 6 : 2) * 86400 },
        fetchedAt: new Date(now - (mode === "stale" ? 42 * 60_000 : 25_000)).toISOString(),
        error: mode === "error" ? "HTTP 503 Service Unavailable" : null,
      },
    };
  });
}

export function CapacityPopover() {
  const { language } = useLanguage();
  const copy = COPY[language];
  const query = new URLSearchParams(window.location.search);
  const previewMode = query.get("preview");
  const [page, setPage] = useState<PopoverPage>(
    query.get("panel") === "plan" || query.get("panel") === "accounts"
      ? query.get("panel") as PopoverPage
      : "capacity",
  );
  const [envelope, setEnvelope] = useState<DesktopStatusEnvelope>(() => getCapacityStatusSnapshot() ?? previewEnvelope(previewMode));
  const [workPlan, setWorkPlan] = useState<DesktopWorkPlanEnvelope>(() => previewWorkPlan(previewMode));
  const [busy, setBusy] = useState(hasLocalBackend);
  const [refreshNote, setRefreshNote] = useState<ReturnType<typeof capacityRefreshFeedback> | null>(null);
  const [scheduleBusy, setScheduleBusy] = useState(false);
  const [scheduleNote, setScheduleNote] = useState<string | null>(null);
  const accountOverview = useAccountOverviews(page === "accounts", previewAccounts(previewMode));
  const { accounts, privacyMode, busy: accountsBusy, progress: accountRefreshProgress,
    result: accountRefreshResult, reload: reloadAccounts, refreshAll: refreshAccounts } = accountOverview;
  const [now, setNow] = useState(Date.now());
  const publicSources = useSyncExternalStore(publicSourcesRuntime.subscribe, publicSourcesRuntime.getSnapshot, publicSourcesRuntime.getSnapshot);
  const resetNotice = useMemo(() => upcomingResetNotices(publicSources.timeline, now)[0], [publicSources.timeline, now]);
  const resetUpdatesCount = useMemo(() => unreadResetProgress(publicSources.timeline, now).length, [publicSources.timeline, now]);
  const [resetOpenFailed, setResetOpenFailed] = useState(false);
  const workPlanRequestId = useRef(0);
  const scheduleBusyRef = useRef(false);
  const refreshInFlight = useRef<Promise<void> | null>(null);

  const applyEnvelope = useCallback((incoming: DesktopStatusEnvelope) => {
    setEnvelope((current) => acceptNewerEnvelope(current, incoming));
  }, []);

  const reloadWorkPlan = useCallback(async () => {
    if (!hasLocalBackend || scheduleBusyRef.current) return null;
    const requestId = ++workPlanRequestId.current;
    try {
      const plan = await getCapacityWorkPlan();
      if (requestId === workPlanRequestId.current) setWorkPlan(plan);
      return plan;
    } catch {
      return null;
    }
  }, []);

  const refresh = useCallback((manual = false): Promise<void> => {
    if (!hasLocalBackend) return Promise.resolve();
    if (refreshInFlight.current) return refreshInFlight.current;
    setBusy(true);
    setRefreshNote(null);
    const request = (async () => {
      try {
        const result = await refreshCapacityStatus();
        applyEnvelope(result);
        if (manual) setRefreshNote(capacityRefreshFeedback(result, language));
        await reloadWorkPlan();
      } catch {
        if (manual) setRefreshNote({ kind: "warning", message: capacityIssueMessage(undefined, language) });
        // Keep the last snapshot. The backend retains its original observation
        // time and reports stale/error instead of turning failures into 100%.
        try { applyEnvelope(await getCapacityStatus()); } catch { /* offline */ }
      } finally {
        refreshInFlight.current = null;
        setBusy(false);
      }
    })();
    refreshInFlight.current = request;
    return request;
  }, [applyEnvelope, reloadWorkPlan, language]);

  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => window.clearInterval(timer);
  }, []);

  useEffect(() => {
    if (!hasLocalBackend) return;
    let active = true;
    const check = () => {
      void getCurrentWindow().isVisible().then((visible) => {
        if (active && visible) void publicSourcesRuntime.refresh(false);
      }).catch(() => undefined);
    };
    check();
    const timer = window.setInterval(check, 15 * 60_000);
    return () => { active = false; window.clearInterval(timer); };
  }, []);

  useEffect(() => {
    if (!hasLocalBackend) return;
    let active = true;
    void getCapacityCachedStatus().then((cached) => {
      if (active && cached) applyEnvelope(cached);
    }).catch(() => undefined);
    void getCapacityStatus()
      .then(async (current) => {
        if (!active) return;
        applyEnvelope(current);
        await refresh();
        if (active) await reloadWorkPlan();
      })
      .catch(() => undefined)
      .finally(() => { if (active) setBusy(false); });
    const unsubscribe = subscribeToCapacityStatus((status) => {
      if (!active) return;
      applyEnvelope(status);
      void reloadWorkPlan();
    });
    return () => { active = false; unsubscribe(); };
  }, [applyEnvelope, refresh, reloadWorkPlan]);

  useEffect(() => {
    let active = true;
    const unsubscribeAccounts = subscribeToBackendEvents(
      () => {
        void reloadWorkPlan();
      },
      () => undefined,
    );
    let unsubscribeFocus: (() => void) | undefined;
    if (hasLocalBackend) {
      void getCurrentWindow().onFocusChanged(({ payload: focused }) => {
        if (focused) {
          setNow(Date.now());
          void publicSourcesRuntime.refresh(false);
          void reloadAccounts();
          void reloadWorkPlan();
          void refresh();
        }
      }).then((unsubscribe) => {
        if (active) unsubscribeFocus = unsubscribe;
        else unsubscribe();
      }).catch(() => undefined);
    }
    return () => {
      active = false;
      unsubscribeAccounts();
      unsubscribeFocus?.();
    };
  }, [refresh, reloadWorkPlan, reloadAccounts]);

  useEffect(() => {
    if (!hasLocalBackend || page !== "plan") return;
    void reloadWorkPlan();
    const timer = window.setInterval(() => void reloadWorkPlan(), 30_000);
    return () => window.clearInterval(timer);
  }, [page, reloadWorkPlan]);

  const saveSchedule = useCallback(async (enabled: boolean, offPeriods: DesktopWorkSchedulePeriod[]) => {
    const current = workPlan.schedule;
    if (!current || scheduleBusy) return;
    scheduleBusyRef.current = true;
    setScheduleBusy(true);
    setScheduleNote(null);
    if (!hasLocalBackend) {
      setWorkPlan((plan) => ({
        ...plan,
        status: "updated",
        schedule: plan.schedule ? { ...plan.schedule, revision: plan.schedule.revision + 1, enabled, offPeriods } : null,
        baseline: plan.baseline ? { ...plan.baseline, comparison: { ...plan.baseline.comparison, enabled } } : null,
      }));
      setScheduleNote(copy.synced);
      scheduleBusyRef.current = false;
      setScheduleBusy(false);
      return;
    }
    try {
      const requestId = ++workPlanRequestId.current;
      const result = await updateCapacityWorkSchedule({ expectedRevision: current.revision, enabled, offPeriods });
      if (requestId === workPlanRequestId.current && (result.status === "updated" || result.status === "revision_conflict")) {
        setWorkPlan(result);
        setScheduleNote(result.status === "revision_conflict" ? copy.conflict : copy.synced);
      } else {
        setScheduleNote(copy.saveFailed);
      }
    } catch {
      setScheduleNote(copy.saveFailed);
    } finally {
      scheduleBusyRef.current = false;
      setScheduleBusy(false);
      void reloadAccounts();
    }
  }, [copy.conflict, copy.saveFailed, copy.synced, scheduleBusy, workPlan.schedule, reloadAccounts]);

  const openAccount = useCallback(async (accountId: string) => {
    try {
      if (hasLocalBackend) await requestAccountSwitchFromPopover(accountId);
      else await showDashboardFromBubble();
      await getCurrentWindow().hide().catch(() => undefined);
    } catch {
      // Keep the popover open when the dashboard cannot accept the request.
    }
  }, []);

  const openDashboard = useCallback(async (settings = false) => {
    if (settings) await showSettingsFromPopover();
    else await showDashboardFromBubble();
    await getCurrentWindow().hide().catch(() => undefined);
  }, []);

  const openResetIntelligence = useCallback(async () => {
    setResetOpenFailed(false);
    try {
      await showResetIntelligenceFromPopover();
      await getCurrentWindow().hide().catch(() => undefined);
    } catch { setResetOpenFailed(true); }
  }, []);

  const status = envelope.status;
  const planType = status?.account?.planType?.toUpperCase() || "CHATGPT";
  const windows = useMemo(
    () => popoverWindows(status?.quotaWindows ?? [], status?.account?.planType),
    [status?.account?.planType, status?.quotaWindows],
  );
  const remaining = windows.primary?.remainingPercent ?? null;
  const tone = capacityTone(remaining);
  const notice = envelope.issue ? capacityQuotaNotice(envelope, busy, language)
    : refreshNote ?? capacityQuotaNotice(envelope, busy, language);
  const freshness = status?.quotaFreshness ?? status?.dataStatus.freshness;
  const statusLabel = status?.quotaSource === "managed_account_cache"
    ? copy.stale
    : freshness === "live" ? copy.live : freshness === "stale" ? copy.stale : envelope.lifecycle === "selection_required" ? copy.partial : copy.unavailable;
  const ringStyle = { "--capacity-progress": `${remaining ?? 0}%` } as CSSProperties;
  const schedule = workPlan.schedule;
  const baseline = workPlan.baseline?.comparison ?? null;
  const baselineObservation = workPlan.baseline;
  const baselineSourceLabel = baselineObservation?.source === "managed_account_cache"
    ? copy.planCachedSource
    : baselineObservation?.freshness === "live"
      ? copy.planLiveSource
      : copy.planSnapshotSource;
  const baselineObservationNote = baselineObservation
    ? `${baselineSourceLabel} · ${updatedLabel(baselineObservation.observedAt, now, language)}`
    : null;
  const planTone = paceTone(baseline?.state);
  const margin = baseline ? baseline.actualRemainingPercent - baseline.expectedRemainingPercent : null;

  const updatePeriod = (index: number, field: keyof DesktopWorkSchedulePeriod, value: string) => {
    if (!schedule) return;
    const minute = timeValueToMinute(value);
    if (minute === null) return;
    const periods = schedule.offPeriods.map((period, periodIndex) => periodIndex === index ? { ...period, [field]: minute } : period);
    void saveSchedule(schedule.enabled, periods);
  };

  return (
    <main className={styles.popover} data-tone={tone} data-plan-tone={planTone}>
      <header className={styles.header}>
        <div className={styles.identity}>
          <span className={styles.logo}><Gauge size={17} strokeWidth={2.2} /></span>
          <span className={styles.providerIdentity}><small>{copy.official}</small><strong>Codex <b>{planType}</b></strong></span>
        </div>
        <button type="button" className={styles.freshnessAction} onClick={() => void refresh(true)} disabled={busy || !hasLocalBackend} title={copy.refresh}>
          <i /><span>{statusLabel}</span><small>{updatedLabel(status?.quotaObservedAt ?? status?.capturedAt, now, language)}</small>
          <RefreshCw size={12} className={busy ? styles.spinning : undefined} />
        </button>
      </header>

      <nav className={styles.pageTabs} aria-label="Capacity popover pages">
        <button type="button" role="tab" aria-selected={page === "capacity"} onClick={() => setPage("capacity")}><Gauge size={13} />{copy.capacity}</button>
        <button type="button" role="tab" aria-selected={page === "plan"} onClick={() => setPage("plan")}><Briefcase size={13} />{copy.plan}</button>
        <button type="button" role="tab" aria-selected={page === "accounts"} onClick={() => setPage("accounts")}><UsersRound size={13} />{copy.accounts}</button>
      </nav>

      <div className={styles.pageViewport}>
        {page === "capacity" && (status && windows.primary ? (
          <section className={styles.capacityPage} aria-label={copy.capacity}>
            <div className={styles.hero}>
              <div className={styles.ring} style={ringStyle}><div><strong>{percentLabel(windows.primary)}</strong><span>{copy.remaining}</span></div></div>
              <div className={styles.heroCopy}>
                <span className={styles.windowLabel}>{durationLabel(windows.primary, language)}</span>
                <strong>{resetCountdown(windows.primary.resetsAt, now, language)}</strong>
                <small><Clock3 size={12} /> {localResetTime(windows.primary.resetsAt, language)}</small>
              </div>
            </div>
            <ShortQuotaCard window={windows.secondary} language={language} now={now}
              notApplicable={planHasNoShortQuotaWindow(status.account?.planType)} />
            <div className={styles.facts}>
              <div><span><ShieldCheck size={13} />{copy.account}</span><strong>{status.account?.planType || "ChatGPT"}</strong></div>
              <div><span>{copy.source}</span><strong>{updatedLabel(status.quotaObservedAt ?? status.capturedAt, now, language)}</strong></div>
            </div>
            {notice && (
              <div className={styles.notice} data-kind={notice.kind} role="status">
                {notice.kind === "warning" ? <TriangleAlert size={13} /> : <Clock3 size={13} />}<span>{notice.message}</span>
              </div>
            )}
          </section>
        ) : (
          <section className={styles.emptyState}>
            <span><Gauge size={25} /></span>
            <strong>{busy ? copy.waiting : envelope.lifecycle === "selection_required" ? copy.select : copy.noSnapshot}</strong>
            {notice && <small role="status">{notice.message}</small>}
            <button type="button" onClick={() => envelope.lifecycle === "selection_required" ? void openDashboard() : void refresh(true)} disabled={busy || (!hasLocalBackend && !previewMode)}>
              {envelope.lifecycle === "selection_required" ? <>{copy.open}<ArrowUpRight size={14} /></> : <>{copy.retry}<RefreshCw size={14} /></>}
            </button>
          </section>
        ))}

        {page === "plan" && (
          <section className={styles.planPage} aria-label={copy.plan}>
            <div className={styles.planMode}>
              <span className={styles.planIcon}>{baseline?.segment === "off" ? <Pause size={15} /> : <Briefcase size={15} />}</span>
              <span><strong>{copy.planTitle}</strong><small>{copy.planSubtitle}</small></span>
              <button type="button" className={styles.switch} role="switch" aria-checked={schedule?.enabled ?? false} disabled={!schedule || scheduleBusy} onClick={() => schedule && void saveSchedule(!schedule.enabled, schedule.offPeriods)} title={schedule?.enabled ? copy.enabled : copy.disabled}><i /></button>
            </div>

            {baseline ? (
              <>
                <div className={styles.paceCard}>
                  <div className={styles.paceHeading}>
                    <span>{baseline.state === "on_pace" ? copy.onPace : baseline.state === "within_guard" ? copy.withinGuard : copy.overGuard}</span>
                    <strong>{margin !== null && margin >= 0 ? `${margin.toFixed(1)}% ${copy.ahead}` : `${Math.abs(margin ?? 0).toFixed(1)}% ${copy.overBy}`}</strong>
                  </div>
                  <div className={styles.paceValues}>
                    <span><small>{copy.actual}</small><strong>{percentValueLabel(baseline.actualRemainingPercent)}</strong></span>
                    <span><small>{copy.target}</small><strong>{planPercentLabel(baseline.expectedRemainingPercent)}</strong></span>
                  </div>
                  <div className={styles.paceRail}><i style={{ width: `${baseline.actualRemainingPercent}%` }} /><b style={{ left: `${baseline.expectedRemainingPercent}%` }} /></div>
                </div>

                <div className={styles.planStats}>
                  <div><small>{baseline.segment === "off" ? copy.off : copy.working}</small><strong>{baseline.segment === "off" ? copy.frozenAt : copy.nextStop}</strong><span>{targetTimeLabel(baseline.targetAt, language)}</span></div>
                  <div><small>{copy.dailyBudget}</small><strong>{baseline.dailyBudgetPercent.toFixed(1)}%</strong><span>{minuteSpanLabel(baseline.usableMinutesPerDay, language)} {copy.usable}</span></div>
                </div>
              </>
            ) : (
              <div className={styles.planEmpty}><TriangleAlert size={16} /><span>{schedule ? copy.planUnavailable : copy.scheduleUnavailable}</span></div>
            )}

            {schedule && (
              <div className={styles.scheduleEditor}>
                <div className={styles.scheduleHeading}>
                  <strong>{copy.offPeriods}</strong>
                  <button type="button" disabled={scheduleBusy || schedule.offPeriods.length >= 16} onClick={() => void saveSchedule(schedule.enabled, [...schedule.offPeriods, { startMinuteOfDay: 120, endMinuteOfDay: 600 }])}><Plus size={12} />{copy.addPeriod}</button>
                </div>
                {schedule.offPeriods.length ? (
                  <div className={styles.periodList}>
                    {schedule.offPeriods.map((period, index) => (
                      <div className={styles.periodRow} key={`${index}-${period.startMinuteOfDay}-${period.endMinuteOfDay}`}>
                        <Pause size={12} />
                        <input type="time" value={minuteToTimeValue(period.startMinuteOfDay)} disabled={scheduleBusy} onChange={(event) => updatePeriod(index, "startMinuteOfDay", event.target.value)} aria-label={`${copy.offPeriods} ${index + 1} start`} />
                        <span>→</span>
                        <input type="time" value={minuteToTimeValue(period.endMinuteOfDay)} disabled={scheduleBusy} onChange={(event) => updatePeriod(index, "endMinuteOfDay", event.target.value)} aria-label={`${copy.offPeriods} ${index + 1} end`} />
                        <button type="button" disabled={scheduleBusy} onClick={() => void saveSchedule(schedule.enabled, schedule.offPeriods.filter((_, periodIndex) => periodIndex !== index))} aria-label="Remove period"><Trash2 size={12} /></button>
                      </div>
                    ))}
                  </div>
                ) : <small className={styles.allDay}>{copy.allDay}</small>}
                <div className={styles.scheduleMeta}><span>{scheduleNote || baselineObservationNote || copy.baselineNote}</span>{scheduleBusy && <RefreshCw size={11} className={styles.spinning} />}</div>
              </div>
            )}
          </section>
        )}

        {page === "accounts" && (
          <section className={styles.accountsPage} aria-label={copy.accounts}>
            <div className={styles.accountsHeading}>
              <span>
                <strong>{copy.savedAccounts}</strong>
                <small>{language === "zh" ? `${accounts.length} 个账号` : `${accounts.length} accounts`}</small>
              </span>
              <button type="button" disabled={accountsBusy || !accounts.length}
                onClick={() => void refreshAccounts()}>
                <RefreshCw size={12} className={accountsBusy ? styles.spinning : undefined} />
                {accountRefreshProgress
                  ? `${accountRefreshProgress.completed}/${accountRefreshProgress.total}`
                  : copy.refreshAccounts}
              </button>
            </div>
            <p className={styles.accountListNote}>{language === "zh" ? "同一日程 · 各账号独立规划 · 查看无需切换" : "Shared schedule · independent plans · no account switch"}</p>
            {accountOverview.loadFailed && <div className={styles.accountCardWarning} role="status">{language === "zh" ? "账号列表暂未更新，保留现有内容并自动重试" : "Overview temporarily unavailable; keeping the last view and retrying"}</div>}
            {accountRefreshResult && (
              <div className={styles.accountRefreshResult} data-failed={accountRefreshResult.failures.length > 0}>
                {language === "zh"
                  ? `${accountRefreshResult.succeededIds.length} 个已更新 · ${accountRefreshResult.failures.length} 个失败`
                  : `${accountRefreshResult.succeededIds.length} updated · ${accountRefreshResult.failures.length} failed`}
              </div>
            )}
            {accounts.length ? (
              <div className={styles.accountList}>
                {accounts.map((account) => <AccountOverviewCard key={account.id} account={account}
                  privacyMode={privacyMode} now={now} language={language}
                  onSwitch={(id) => void openAccount(id)} />)}
              </div>
            ) : (
              <div className={styles.accountEmpty}>
                <UsersRound size={18} />
                <span>{accountsBusy ? copy.waiting : copy.accountsEmpty}</span>
              </div>
            )}
          </section>
        )}
      </div>

      <footer className={styles.footer}>
        <button type="button" className={styles.primaryAction} onClick={() => void openDashboard()}>{copy.dashboard}<ArrowUpRight size={13} /></button>
        {resetNotice || resetUpdatesCount > 0 || publicSources.timeline?.insights?.forecast.value ? <ResetNoticeChip newUpdates={resetUpdatesCount} notice={resetNotice ? { ...resetNotice, stale: resetNotice.stale || publicSources.failed } : undefined} now={now} language={language}
          insights={publicSources.timeline?.insights} timeline={publicSources.timeline} sourceFailed={publicSources.failed} failed={resetOpenFailed} onOpen={() => void openResetIntelligence()} />
          : <div className={styles.pageDots} aria-hidden="true"><i data-active={page === "capacity"} /><i data-active={page === "plan"} /><i data-active={page === "accounts"} /></div>}
        <button type="button" className={styles.secondaryAction} onClick={() => void openDashboard(true)} title={copy.settings}><Settings2 size={14} /></button>
      </footer>
    </main>
  );
}
