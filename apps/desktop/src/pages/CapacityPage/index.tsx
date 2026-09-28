import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Alert, Button, Select, Skeleton, Tag } from "antd";
import {
  RefreshCw,
  Ticket,
} from "lucide-react";
import type {
  DesktopDiagnosticsEnvelope,
  DesktopQuotaWindow,
  DesktopStatusEnvelope,
  DesktopVaultMutationStatusEnvelope,
} from "../../../../capacity-preview/src/status";
import {
  getCapacityDiagnostics,
  authorizeCapacityHistoryKeychain,
  getCapacityStatus,
  getCapacityStatusSnapshot,
  getCapacityCachedStatus,
  getCapacityVaultStatus,
  hasLocalBackend,
  isDesktopApp,
  refreshCapacityStatus,
  selectCapacityCodexExecutable,
  subscribeToCapacityStatus,
} from "../../api/backend";
import type { Language } from "../../i18n";
import { quotaPercentLabel } from "../../utils/quotaPercent";
import {
  clampPercent,
  selectPrimaryQuotaWindow,
  selectShortestQuotaWindow,
} from "./presentation";
import styles from "./index.module.less";
import { acceptNewerEnvelope } from "../../components/CapacityPopover/presentation";
import { planHasNoShortQuotaWindow } from "../../utils/accountUsageWindows";
import { QuotaGauge } from "./QuotaGauge";
import { CapacityHistoryPanel } from "./CapacityHistoryPanel";
import { resetCreditPresentation } from "./resetCreditPresentation";
import { CapacityDetails } from "./CapacityDetails";
import { capacityIssueMessage, capacityPlanLabel, capacityRefreshFeedback } from "./statusCopy";
import { currentInstallationChoice, installationIssueCode, installationLabel, selectableInstallation } from "./installationSelection";

interface CapacityPageProps {
  language: Language;
  dark?: boolean;
  notify: (message: string) => void;
}

const COPY = {
  en: {
    intro: "Your quota and reset times at a glance.",
    introDetail: "Official account quota updates automatically. History stays on this device.",
    refresh: "Refresh capacity",
    refreshing: "Refreshing",
    official: "Official Codex",
    live: "Live",
    stale: "Stale",
    cachedAccount: "Account snapshot",
    unavailable: "Unavailable",
    authenticationRequired: "Sign in to ChatGPT/Codex to read the official quota snapshot.",
    planningHorizon: "Planning horizon",
    shortWindow: "Short window",
    remaining: "remaining",
    reset: "Resets",
    duration: "Window",
    unknown: "Unknown",
    weekly: "Weekly capacity",
    hourly: "Short-term capacity",
    noShortLimit: "No separate 5h limit on this plan",
    shortUnavailable: "This window is not available yet",
    credits: "Reset credits",
    selectionTitle: "Choose the Codex quota reader",
    selectionBody: "Choose the App’s bundled reader if you use Codex on desktop, or CLI if you use it in a terminal.",
    choose: "Choose an installation",
    confirm: "Use this installation",
    desktopOnly: "Capacity reads require the local QuotaHorizon app or its loopback web view.",
  },
  zh: {
    intro: "额度与重置时间，一目了然。",
    introDetail: "自动更新当前账号的官方额度，历史仅保存在本机。",
    refresh: "刷新额度",
    refreshing: "正在刷新",
    official: "官方 Codex",
    live: "实时",
    stale: "已过期",
    cachedAccount: "账号额度快照",
    unavailable: "暂不可用",
    authenticationRequired: "请先登录 ChatGPT/Codex，随后即可读取官方额度快照。",
    planningHorizon: "规划周期",
    shortWindow: "短周期",
    remaining: "剩余",
    reset: "重置时间",
    duration: "额度窗口",
    unknown: "未知",
    weekly: "周额度",
    hourly: "短期额度",
    noShortLimit: "此套餐无独立 5h 限额",
    shortUnavailable: "尚未取得此窗口额度",
    credits: "重置卡",
    selectionTitle: "选择 Codex 额度读取来源",
    selectionBody: "使用 Codex 桌面应用请选择「App 内置读取组件」；使用终端版本请选择 CLI。",
    choose: "选择 Codex 安装",
    confirm: "使用此安装",
    desktopOnly: "额度读取需要本地 QuotaHorizon 应用或其回环 Web 视图。",
  },
} as const;

function formatTimestamp(value: string | null | undefined, language: Language) {
  if (!value) return COPY[language].unknown;
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return COPY[language].unknown;
  return new Intl.DateTimeFormat(language === "zh" ? "zh-CN" : "en-US", {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
    timeZoneName: "short",
  }).format(date);
}

function formatDuration(minutes: number | null, language: Language) {
  if (minutes == null) return COPY[language].unknown;
  if (minutes >= 1_440 && minutes % 1_440 === 0) {
    const days = minutes / 1_440;
    return language === "zh" ? `${days} 天` : `${days} day${days === 1 ? "" : "s"}`;
  }
  if (minutes >= 60 && minutes % 60 === 0) {
    const hours = minutes / 60;
    return language === "zh" ? `${hours} 小时` : `${hours} hour${hours === 1 ? "" : "s"}`;
  }
  return language === "zh" ? `${minutes} 分钟` : `${minutes} minutes`;
}

function quotaTitle(window: DesktopQuotaWindow, fallback: string) {
  const label = window.label?.trim();
  if (!label || /^(weekly|week|5[- ]?hour)$/i.test(label)) return fallback;
  return label;
}

function QuotaCard({
  language,
  window,
  primary,
  notApplicable = false,
}: {
  language: Language;
  window: DesktopQuotaWindow | null;
  primary: boolean;
  notApplicable?: boolean;
}) {
  const copy = COPY[language];
  const remaining = window ? clampPercent(window.remainingPercent) : null;
  return (
    <article className={`${styles.quotaCard} ${primary ? styles.quotaCardPrimary : ""}`}>
      <QuotaGauge remaining={remaining} label={primary ? copy.weekly : "5h"} />
      <div className={styles.quotaCopy}>
        <span className={styles.quotaKicker}>{primary ? copy.planningHorizon : copy.shortWindow}</span>
        <h2>{window ? quotaTitle(window, primary ? copy.weekly : copy.hourly) : primary ? copy.weekly : "5h"}</h2>
        <dl className={styles.quotaMeta}>
          <div><dt>{copy.remaining}</dt><dd>{quotaPercentLabel(remaining)}</dd></div>
          <div><dt>{copy.duration}</dt><dd>{formatDuration(window ? window.windowMinutes : primary ? null : 300, language)}</dd></div>
          <div><dt>{copy.reset}</dt><dd>{window ? formatTimestamp(window.resetsAt, language) : "—"}</dd></div>
        </dl>
        {!window && <small className={styles.quotaExplanation}>{notApplicable ? copy.noShortLimit : copy.shortUnavailable}</small>}
      </div>
    </article>
  );
}

export function CapacityPage({ language, notify, dark = false }: CapacityPageProps) {
  const copy = COPY[language];
  const [envelope, setEnvelope] = useState<DesktopStatusEnvelope | null>(getCapacityStatusSnapshot);
  const [diagnostics, setDiagnostics] = useState<DesktopDiagnosticsEnvelope | null>(null);
  const [vault, setVault] = useState<DesktopVaultMutationStatusEnvelope | null>(null);
  const [selectedCandidateId, setSelectedCandidateId] = useState<string>();
  const currentCandidateId = currentInstallationChoice(envelope, selectedCandidateId);
  const [busy, setBusy] = useState(true);
  const [authorizingHistory, setAuthorizingHistory] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [refreshNote, setRefreshNote] = useState<ReturnType<typeof capacityRefreshFeedback> | null>(null);
  const auxiliaryGeneration = useRef(0);
  const auxiliaryBusy = useRef(false);

  const loadAuxiliary = useCallback(async () => {
    if (!hasLocalBackend || auxiliaryBusy.current) return;
    auxiliaryBusy.current = true;
    const request = ++auxiliaryGeneration.current;
    const [nextDiagnostics, nextVault] = await Promise.allSettled([
      getCapacityDiagnostics(),
      getCapacityVaultStatus(),
    ]);
    auxiliaryBusy.current = false;
    if (request !== auxiliaryGeneration.current) return;
    setDiagnostics(nextDiagnostics.status === "fulfilled" ? nextDiagnostics.value : null);
    setVault(nextVault.status === "fulfilled" ? nextVault.value : null);
  }, []);

  const load = useCallback(async (forceRefresh: boolean) => {
    if (!hasLocalBackend) {
      setLoadError(copy.desktopOnly);
      setBusy(false);
      return;
    }
    setBusy(true);
    setLoadError(null);
    if (forceRefresh) setRefreshNote(null);
    try {
      // Manual refresh must not wait for a separate status read first.
      let next = forceRefresh ? await refreshCapacityStatus() : await getCapacityStatus();
      if (!forceRefresh && next.lifecycle === "idle") next = await refreshCapacityStatus();
      setEnvelope((current) => current ? acceptNewerEnvelope(current, next) : next);
      if (forceRefresh) setRefreshNote(capacityRefreshFeedback(next, language));
      await loadAuxiliary();
    } catch (error) {
      const message = capacityIssueMessage(undefined, language);
      setLoadError(message);
      notify(message);
    } finally {
      setBusy(false);
    }
  }, [copy.desktopOnly, language, loadAuxiliary, notify]);

  useEffect(() => {
    const inspect = () => { if (document.visibilityState === "visible") void loadAuxiliary(); };
    const timer = window.setInterval(inspect, 30_000);
    window.addEventListener("focus", inspect);
    document.addEventListener("visibilitychange", inspect);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener("focus", inspect);
      document.removeEventListener("visibilitychange", inspect);
      auxiliaryGeneration.current += 1;
    };
  }, [loadAuxiliary]);

  useEffect(() => {
    if (!hasLocalBackend) return;
    let active = true;
    void getCapacityCachedStatus().then((cached) => {
      if (active && cached) setEnvelope((current) => current ? acceptNewerEnvelope(current, cached) : cached);
    }).catch(() => undefined);
    return () => { active = false; };
  }, []);

  useEffect(() => {
    void load(false);
  }, [load]);

  useEffect(() => subscribeToCapacityStatus((next) => {
    setEnvelope((current) => current ? acceptNewerEnvelope(current, next) : next);
  }), []);

  const historyLimitId = selectPrimaryQuotaWindow(envelope?.status?.quotaWindows ?? [])?.limitId;

  const chooseExecutable = useCallback(async () => {
    if (!currentCandidateId) return;
    setBusy(true);
    setLoadError(null);
    try {
      const next = await selectCapacityCodexExecutable(currentCandidateId);
      setEnvelope((current) => current ? acceptNewerEnvelope(current, next) : next);
      if (next.issue && (next.lifecycle === "selection_required" || next.lifecycle === "error")) {
        setLoadError(capacityIssueMessage(next.issue.code, language));
      }
      await loadAuxiliary();
    } catch (error) {
      const message = capacityIssueMessage(installationIssueCode(error), language);
      setLoadError(message);
      notify(message);
    } finally {
      setBusy(false);
    }
  }, [language, loadAuxiliary, notify, currentCandidateId]);

  const authorizeHistory = useCallback(async () => {
    if (authorizingHistory) return;
    setAuthorizingHistory(true);
    try {
      const next = await authorizeCapacityHistoryKeychain();
      setEnvelope((current) => current ? acceptNewerEnvelope(current, next) : next);
    } catch {
      notify(language === "zh" ? "历史授权未完成，可再次点击授权。" : "History access was not authorized. You can try again.");
    } finally {
      setAuthorizingHistory(false);
    }
  }, [authorizingHistory, language, notify]);

  const status = envelope?.status;
  const resetCards = resetCreditPresentation(envelope, language);
  const activeReader = envelope?.selectedExecutableSource === "desktop_app"
    ? (language === "zh" ? "Codex 桌面 App · 内置读取组件" : "Codex desktop App · bundled reader")
    : envelope?.selectedExecutableSource === "cli" ? "Codex CLI" : null;
  const primaryWindow = useMemo(
    () => selectPrimaryQuotaWindow(status?.quotaWindows ?? []),
    [status?.quotaWindows],
  );
  const shortestWindow = useMemo(
    () => selectShortestQuotaWindow(status?.quotaWindows ?? []),
    [status?.quotaWindows],
  );
  const secondaryWindow = planHasNoShortQuotaWindow(status?.account?.planType)
    || shortestWindow?.limitId === primaryWindow?.limitId ? null : shortestWindow;
  const quotaFreshness = status?.quotaFreshness ?? status?.dataStatus.freshness;
  const authenticationRequired = status?.dataStatus.reasonCodes.includes("authentication_required") ?? false;

  return (
    <div className={styles.page}>
      <div className={styles.commandBar}>
        <div className={styles.commandCopy}>
          <strong>{copy.intro}</strong>
          <span>{copy.introDetail}</span>
          <div className={styles.statusStrip}>
            <Tag color="cyan">{copy.official}</Tag>
            {status && quotaFreshness !== "not_applicable" && (
              <Tag color={quotaFreshness === "live" ? "green" : "orange"}>
                {status.quotaSource === "managed_account_cache" ? copy.cachedAccount
                  : quotaFreshness === "live" ? copy.live : copy.stale}
              </Tag>
            )}
            {status?.dataStatus.availability === "unsupported" && (
              <Tag color="gold">{copy.unavailable}</Tag>
            )}
            {status?.account?.planType && <Tag>{capacityPlanLabel(status.account.planType, language)}</Tag>}
          </div>
          {activeReader && <span>{language === "zh" ? "读取来源：" : "Quota reader: "}{activeReader}{status?.codexVersion ? ` · ${status.codexVersion.replace(/^codex-cli\s+/u, "")}` : ""}</span>}
        </div>
        <Button type="primary" icon={<RefreshCw className={busy ? "spin" : undefined} size={15} />}
          loading={busy} disabled={!hasLocalBackend} onClick={() => void load(true)}>
          {busy ? copy.refreshing : copy.refresh}
        </Button>
      </div>

      {loadError && <Alert type="error" showIcon message={loadError} />}
      {refreshNote && !envelope?.issue && <Alert type={refreshNote.kind} showIcon message={refreshNote.message} />}
      {envelope?.issue && loadError !== capacityIssueMessage(envelope.issue.code, language) && <Alert type={status ? "warning" : "error"} showIcon
        message={capacityIssueMessage(envelope.issue.code, language)} />}
      {authenticationRequired && <Alert type="warning" showIcon
        message={copy.authenticationRequired} />}

      {envelope?.lifecycle === "selection_required" && (
        <section className={styles.selection}>
          <h3>{copy.selectionTitle}</h3>
          <p>{copy.selectionBody}</p>
          <div className={styles.selectionActions}>
            <Select value={currentCandidateId} placeholder={copy.choose}
              onChange={setSelectedCandidateId}
              options={envelope.candidates.map((candidate) => ({
                value: candidate.executableId,
                label: installationLabel(candidate, language),
                disabled: !selectableInstallation(candidate),
              }))} />
            <Button type="primary" disabled={!currentCandidateId} loading={busy}
              onClick={() => void chooseExecutable()}>{copy.confirm}</Button>
          </div>
          <p className={styles.installationPath}>{envelope.candidates.find((candidate) => candidate.executableId === currentCandidateId)?.canonicalPath}</p>
        </section>
      )}

      {busy && !envelope ? (
        <div className={styles.skeleton}><Skeleton active /><Skeleton active /></div>
      ) : (
        <>
          <section className={styles.quotaGrid} aria-label={copy.intro}>
            <QuotaCard language={language} window={primaryWindow} primary />
            <QuotaCard language={language} window={secondaryWindow} primary={false}
              notApplicable={planHasNoShortQuotaWindow(status?.account?.planType)} />
          </section>
          <article className={styles.creditCard}>
            <span className={styles.creditIcon}><Ticket size={28} /></span>
            <div className={styles.quotaCopy}>
              <span className={styles.quotaKicker}>{copy.credits}</span>
              <h2>{resetCards.count === null ? "—" : `${resetCards.count}${language === "zh" ? " 张" : ""}`}</h2>
              <span>{resetCards.label}</span>
              <p className={styles.creditMessage}>{resetCards.message}</p>
            </div>
          </article>

          <CapacityHistoryPanel contextId={envelope?.historyContextId}
            executableId={envelope?.selectedExecutableId} limitId={historyLimitId}
            sequence={envelope?.sequence ?? 0} language={language} dark={dark}
            onAuthorize={isDesktopApp ? authorizeHistory : undefined} authorizing={authorizingHistory} />

          <CapacityDetails envelope={envelope} diagnostics={diagnostics} vault={vault}
            language={language} onInspect={() => void loadAuxiliary()} />
        </>
      )}
    </div>
  );
}
