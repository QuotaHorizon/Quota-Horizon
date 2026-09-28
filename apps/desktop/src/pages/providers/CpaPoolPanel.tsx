import { useCallback, useEffect, useMemo, useState } from "react";
import { Button, Input, InputNumber, Switch, Tag } from "antd";
import { Activity, Database, RefreshCw, Route, Settings2, ShieldAlert } from "lucide-react";
import {
  DEFAULT_CPA_BRIDGE_SETTINGS,
  getCpaPoolStatus,
  loadAppSettings,
  refreshCpaPoolStatus,
  subscribeToCpaPoolStatus,
  updateCpaBridgeSettings,
} from "../../api/backend";
import type { Language, Translate } from "../../i18n";
import type {
  CpaBridgeSettings,
  CpaPoolMember,
  CpaPoolStatus,
  CpaQuotaWindow,
  Provider,
} from "../../types";
import {
  cpaGuardReasonKey, cpaGuardStateKey, cpaIssueKey, cpaRouteKeys, isLoopbackProvider,
  remainingPercent,
} from "./cpaPoolPresentation";

interface CpaPoolPanelProps {
  active: boolean;
  providers: Provider[];
  language: Language;
  t: Translate;
}

function formatTimestamp(value: string | null | undefined, language: Language) {
  if (!value) return "—";
  const timestamp = new Date(value);
  if (Number.isNaN(timestamp.getTime())) return "—";
  return timestamp.toLocaleString(language === "zh" ? "zh-CN" : "en-US", {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function formatReset(value: number | null | undefined, language: Language) {
  if (value === null || value === undefined) return "—";
  return formatTimestamp(new Date(value * 1_000).toISOString(), language);
}

function QuotaCell({ label, value, language, t }: {
  label: string;
  value?: CpaQuotaWindow | null;
  language: Language;
  t: Translate;
}) {
  const remaining = remainingPercent(value?.remainingPercent);
  return (
    <div className={`cpa-quota-cell${remaining ? "" : " unknown"}`}>
      <span>{label}</span>
      <strong>{remaining ?? "—"}</strong>
      <small>{value ? t("providers.cpa.resets", { time: formatReset(value.resetsAt, language) })
        : t("providers.cpa.noQuota")}</small>
      {value && <i aria-hidden="true"><b style={{ width: `${value.remainingPercent}%` }} /></i>}
    </div>
  );
}

function MemberRow({ member, parent, language, t }: {
  member: CpaPoolMember;
  parent: boolean;
  language: Language;
  t: Translate;
}) {
  const routeKeys = cpaRouteKeys(member);
  const guardStateKey = cpaGuardStateKey(member.quotaGuardState);
  const guardReasonKey = cpaGuardReasonKey(member.quotaGuardReason);
  const details = [
    member.model,
    member.reasoningEffort,
    member.planType,
    member.statusCode ? `HTTP ${member.statusCode}` : null,
  ].filter(Boolean);
  return (
    <article className={`cpa-member${parent ? " parent" : ""}${member.isStale ? " stale" : ""}`}>
      <div className="cpa-member-identity">
        <div>
          <strong>{member.displayName}</strong>
          {parent && <Tag color="green">{t("providers.cpa.parent")}</Tag>}
        </div>
        <div className="cpa-route-tags">
          {routeKeys.map((key) => <Tag key={key}>{t(key)}</Tag>)}
          {!routeKeys.length && <span>{t("providers.cpa.route.standby")}</span>}
        </div>
        <small>{details.join(" · ") || t("providers.cpa.awaitingSample")}</small>
      </div>
      <QuotaCell label={t("providers.cpa.primary")} value={member.primary} language={language} t={t} />
      <QuotaCell label={t("providers.cpa.secondary")} value={member.secondary} language={language} t={t} />
      <div className="cpa-member-health">
        {guardStateKey && <span className="guard"><ShieldAlert size={12} />
          {[t(guardStateKey), guardReasonKey ? t(guardReasonKey) : null].filter(Boolean).join(" · ")}
        </span>}
        {member.estimatedRemainingSuccesses !== null
          && member.estimatedRemainingSuccesses !== undefined
          && <span>{t("providers.cpa.estimatedSuccesses", {
            count: member.estimatedRemainingSuccesses,
          })}</span>}
        {member.statsSampleCount !== null && member.statsSampleCount !== undefined
          && <span>{t("providers.cpa.samples", { count: member.statsSampleCount })}</span>}
        {(member.refreshSkipped || member.failed) && <span className="warning">
          {member.refreshSkipped ? t("providers.cpa.refreshSkipped") : t("providers.cpa.lastRequestFailed")}
        </span>}
        <span>{t("providers.cpa.observed", {
          time: formatTimestamp(member.observedAt, language),
        })}</span>
      </div>
    </article>
  );
}

export function CpaPoolPanel({ active, providers, language, t }: CpaPoolPanelProps) {
  const eligible = useMemo(() => providers.some(isLoopbackProvider), [providers]);
  const [status, setStatus] = useState<CpaPoolStatus | null>(null);
  const [bridgeSettings, setBridgeSettings] = useState<CpaBridgeSettings>(DEFAULT_CPA_BRIDGE_SETTINGS);
  const [draft, setDraft] = useState<CpaBridgeSettings>(DEFAULT_CPA_BRIDGE_SETTINGS);
  const [refreshing, setRefreshing] = useState(false);
  const [saving, setSaving] = useState(false);
  const [saveFailed, setSaveFailed] = useState(false);

  const refresh = useCallback(async () => {
    setRefreshing(true);
    try {
      setStatus(await refreshCpaPoolStatus());
    } finally {
      setRefreshing(false);
    }
  }, []);

  useEffect(() => {
    if (!active) return undefined;
    let cancelled = false;
    void Promise.all([getCpaPoolStatus(), loadAppSettings()])
      .then(([cached, settings]) => {
        if (cancelled) return;
        const configured = settings.cpaBridge ?? DEFAULT_CPA_BRIDGE_SETTINGS;
        setStatus(cached);
        setBridgeSettings(configured);
        setDraft(configured);
      })
      .catch(() => {
        if (!cancelled) setSaveFailed(true);
      });
    return () => { cancelled = true; };
  }, [active]);

  useEffect(() => {
    if (!active) return undefined;
    return subscribeToCpaPoolStatus(setStatus);
  }, [active]);

  const persistSettings = useCallback(async (next: CpaBridgeSettings) => {
    setSaving(true);
    setSaveFailed(false);
    try {
      const saved = await updateCpaBridgeSettings(next);
      const configured = saved.cpaBridge ?? next;
      setBridgeSettings(configured);
      setDraft(configured);
    } catch {
      setSaveFailed(true);
    } finally {
      setSaving(false);
    }
  }, []);

  const host = draft.sshHost.trim();
  const validHost = host.length > 0
    && host.length <= 255
    && !host.startsWith("-")
    && /^[A-Za-z0-9._@:-]+$/.test(host);
  const validRefresh = Number.isInteger(draft.refreshSeconds)
    && draft.refreshSeconds >= 60 && draft.refreshSeconds <= 3_600;
  const validTimeout = Number.isInteger(draft.timeoutSeconds)
    && draft.timeoutSeconds >= 2 && draft.timeoutSeconds <= 120;
  const draftValid = (validHost || (!draft.enabled && !host)) && validRefresh && validTimeout;
  const dirty = JSON.stringify(draft) !== JSON.stringify(bridgeSettings);
  const issueText = status?.issue ? t(cpaIssueKey(status.issue.code)) : null;
  const parent = status?.members.find((member) => member.id === status.parentMemberId);
  return (
    <section className="cpa-pool-panel" aria-label={t("providers.cpa.title")}>
      <header>
        <div className="cpa-pool-heading">
          <span className="cpa-pool-icon"><Database size={17} /></span>
          <div>
            <strong>{t("providers.cpa.title")}</strong>
            <small>{parent
              ? t("providers.cpa.summary", { name: parent.displayName, count: status?.members.length ?? 0 })
              : t("providers.cpa.description")}</small>
          </div>
        </div>
        <div className="cpa-pool-actions">
          {status?.source === "live" && <Tag color="green"><Activity size={11} /> {t("providers.cpa.live")}</Tag>}
          {status?.source === "cached" && <Tag color="gold"><Database size={11} /> {t("providers.cpa.cached")}</Tag>}
          <span>{t("providers.cpa.updated", { time: formatTimestamp(status?.fetchedAt, language) })}</span>
          <Button size="small" icon={<RefreshCw size={13} className={refreshing ? "spin" : ""} />}
            disabled={!bridgeSettings.enabled} loading={refreshing}
            onClick={() => void refresh()}>{t("providers.cpa.refresh")}</Button>
        </div>
      </header>
      <div className="cpa-bridge-config">
        <div className="cpa-bridge-config-copy">
          <Settings2 size={15} />
          <div><strong>{t("providers.cpa.bridge.title")}</strong>
            <small>{t("providers.cpa.bridge.description")}</small></div>
        </div>
        <div className="cpa-bridge-fields">
          <label htmlFor="cpa-bridge-enabled">{t("providers.cpa.bridge.enabled")}</label>
          <Switch id="cpa-bridge-enabled" checked={bridgeSettings.enabled} loading={saving}
            disabled={!bridgeSettings.enabled && !bridgeSettings.sshHost.trim()}
            checkedChildren={t("settings.autoRefresh.on")} unCheckedChildren={t("settings.autoRefresh.off")}
            onChange={(enabled) => void persistSettings({ ...bridgeSettings, enabled })} />
          <label htmlFor="cpa-bridge-host">{t("providers.cpa.bridge.host")}</label>
          <Input id="cpa-bridge-host" size="small" value={draft.sshHost} disabled={saving}
            placeholder={t("providers.cpa.bridge.hostPlaceholder")}
            status={validHost || (!draft.enabled && !host) ? undefined : "error"}
            onChange={(event) => setDraft((current) => ({ ...current, sshHost: event.target.value }))} />
          <label htmlFor="cpa-bridge-refresh">{t("providers.cpa.bridge.refreshSeconds")}</label>
          <InputNumber id="cpa-bridge-refresh" size="small" min={60} max={3_600} precision={0}
            value={draft.refreshSeconds} disabled={saving}
            status={validRefresh ? undefined : "error"}
            onChange={(value) => setDraft((current) => ({
              ...current,
              refreshSeconds: typeof value === "number" ? value : 0,
            }))} />
          <label htmlFor="cpa-bridge-timeout">{t("providers.cpa.bridge.timeoutSeconds")}</label>
          <InputNumber id="cpa-bridge-timeout" size="small" min={2} max={120} precision={0}
            value={draft.timeoutSeconds} disabled={saving}
            status={validTimeout ? undefined : "error"}
            onChange={(value) => setDraft((current) => ({
              ...current,
              timeoutSeconds: typeof value === "number" ? value : 0,
            }))} />
          <Button size="small" type="primary" loading={saving} disabled={!dirty || !draftValid}
            onClick={() => void persistSettings(draft)}>{t("providers.cpa.bridge.save")}</Button>
        </div>
        {!eligible && <small className="cpa-bridge-hint">{t("providers.cpa.bridge.noLocalProvider")}</small>}
        {!bridgeSettings.enabled && <small className="cpa-bridge-hint">{t("providers.cpa.bridge.disabled")}</small>}
        {saveFailed && <small className="cpa-bridge-error" role="alert">
          {t("providers.cpa.bridge.saveFailed")}
        </small>}
      </div>
      {issueText && <div className="cpa-pool-issue" role="status">
        <ShieldAlert size={14} />
        <span>{issueText}{status?.members.length ? ` · ${t("providers.cpa.cachedRetained")}` : ""}</span>
      </div>}
      {!status?.members.length ? <div className="cpa-pool-empty">
        <Route size={18} />
        <span>{refreshing ? t("providers.cpa.loading") : t("providers.cpa.empty")}</span>
      </div> : <div className="cpa-member-list">
        {status.members.map((member) => <MemberRow key={member.id} member={member}
          parent={member.id === status.parentMemberId} language={language} t={t} />)}
      </div>}
    </section>
  );
}
