import { Progress } from "antd";
import { useEffect, useState } from "react";
import type { Language, Translate } from "../../i18n";
import type { Account, UsageWindow } from "../../types";
import { accountUsageWindows, planHasNoShortQuotaWindow } from "../../utils/accountUsageWindows";
import { quotaPercentLabel } from "../../utils/quotaPercent";
import { formatSystemTime, remainingTone, resetCountdownTime, resetCountdownWithDays, resetLabel, type UsageResetWindow } from "../../utils/format";

function usageStroke(value: number) {
  const tone = remainingTone(value);
  if (tone === "danger") return "#d2685b";
  if (tone === "warning") return "#d0a340";
  return "var(--green)";
}

function tableResetLabel(timestamp: number | null | undefined, language: Language, resetWindow: UsageResetWindow, now: number) {
  if (timestamp && !Number.isFinite(new Date(timestamp * 1000).getTime())) timestamp = null;
  const label = resetLabel(timestamp, language, resetWindow);
  if (!timestamp) return label;
  if (resetWindow === "oneWeek") {
    const countdown = resetCountdownWithDays(timestamp, language, now);
    if (!countdown) return label;
    return language === "zh" ? `${label} · 剩 ${countdown}` : `${label} · ${countdown} left`;
  }
  const countdown = resetCountdownTime(timestamp, now);
  if (!countdown) return label;
  return language === "zh" ? `${label} · 剩 ${countdown}` : `${label} · ${countdown} left`;
}

function refreshTimeLabel(refreshedAt: number, now: number, t: Translate) {
  if (!Number.isFinite(refreshedAt)) return t("usage.updateTimeUnknown");
  const elapsed = now - refreshedAt;
  if (elapsed < 0) return t("usage.updateTimeAhead");
  if (elapsed < 60_000) return t("usage.updatedJustNow");
  if (elapsed < 3_600_000) return t("usage.updatedMinutesAgo", { count: Math.floor(elapsed / 60_000) });
  if (elapsed < 86_400_000) return t("usage.updatedHoursAgo", { count: Math.floor(elapsed / 3_600_000) });
  return t("usage.updatedDaysAgo", { count: Math.floor(elapsed / 86_400_000) });
}

export function UsageRefreshAge({ fetchedAt, language, t, className = "account-card-refresh-age" }: {
  fetchedAt?: string | null;
  language: Language;
  t: Translate;
  className?: string;
}) {
  const [, refreshAge] = useState(0);
  const refreshedAt = fetchedAt ? Date.parse(fetchedAt) : NaN;
  const validTime = Number.isFinite(refreshedAt);

  useEffect(() => {
    if (!validTime) return;
    const timer = window.setInterval(() => refreshAge((tick) => tick + 1), 5_000);
    return () => window.clearInterval(timer);
  }, [validTime, fetchedAt]);

  return <span className={className} title={validTime ? formatSystemTime(fetchedAt, language) : undefined}>
    {refreshTimeLabel(refreshedAt, Date.now(), t)}
  </span>;
}

interface UsageMeterProps {
  window?: UsageWindow | null;
  resetWindow: UsageResetWindow;
  fetchedAt?: string | null;
  unavailableReason?: string;
  refreshFailed?: boolean;
  variant?: "line" | "card";
  cardLabel?: string;
  cardLabelSuffix?: string;
  language: Language;
  t: Translate;
}

/** Keep window selection and observation metadata together in both account layouts. */
export function AccountUsageMeter({ account, slot, variant = "line", language, t }: {
  account: Pick<Account, "plan" | "usage">;
  slot: "short" | "weekly";
  variant?: "line" | "card";
  language: Language;
  t: Translate;
}) {
  const plan = account.plan || account.usage.plan;
  const missingShortWindow = slot === "short" && planHasNoShortQuotaWindow(plan);
  return <UsageMeter
    window={accountUsageWindows(account.usage, plan)[slot]}
    resetWindow={slot === "short" ? "fiveHours" : "oneWeek"}
    fetchedAt={account.usage.fetchedAt}
    refreshFailed={Boolean(account.usage.error)}
    unavailableReason={missingShortWindow ? t("usage.noShortWindow") : t("usage.windowUnavailable")}
    variant={variant}
    cardLabel={t(slot === "short" ? "table.fiveHours" : "table.oneWeek")}
    language={language} t={t} />;
}

function windowDurationLabel(minutes: number | null | undefined, t: Translate) {
  if (!minutes || !Number.isFinite(minutes) || minutes <= 0) return null;
  if (minutes % (24 * 60) === 0) return t("usage.windowDays", { count: minutes / (24 * 60) });
  if (minutes % 60 === 0) return t("usage.windowHours", { count: minutes / 60 });
  return t("usage.windowMinutes", { count: minutes });
}

export function resolveUsageResetWindow(
  usageWindow: UsageWindow | null | undefined,
  fallback: UsageResetWindow,
): UsageResetWindow {
  const minutes = usageWindow?.windowMinutes;
  if (!minutes || !Number.isFinite(minutes) || minutes <= 0) return fallback;
  return minutes <= 24 * 60 ? "fiveHours" : "oneWeek";
}

export function UsageMeter({
  window: usageWindow,
  resetWindow,
  fetchedAt,
  unavailableReason,
  refreshFailed = false,
  variant = "line",
  cardLabel,
  cardLabelSuffix,
  language,
  t,
}: UsageMeterProps) {
  const [now, setNow] = useState(() => Date.now());
  const effectiveResetWindow = resolveUsageResetWindow(usageWindow, resetWindow);
  const remainingLabel = quotaPercentLabel(usageWindow?.remainingPercent);
  const hasReading = Boolean(usageWindow && remainingLabel !== "—");
  const tickerActive = hasReading && Boolean(usageWindow?.resetsAt);

  useEffect(() => {
    if (!tickerActive) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [tickerActive, usageWindow?.resetsAt, fetchedAt]);

  if (!usageWindow || !hasReading) return (
    <div className={`table-usage usage-unavailable table-usage-${effectiveResetWindow}${variant === "card" ? " card-usage-meter" : ""}`}>
      <div className="table-usage-head">
        <strong className="usage-missing">—</strong>
        {variant === "card" && cardLabel && <span className="card-usage-name">{cardLabel}</span>}
      </div>
      <div className="usage-unavailable-track" aria-hidden="true" />
      <span className="usage-missing">{unavailableReason ?? t("usage.windowUnavailable")}</span>
    </div>
  );
  const remaining = usageWindow.remainingPercent;
  const tone = remainingTone(remaining);
  const duration = windowDurationLabel(usageWindow.windowMinutes, t);
  const failureNotice = refreshFailed && <span className="usage-cache-notice">{t("usage.cachedAfterFailure")}</span>;
  if (variant === "card") return (
    <div className={`table-usage card-usage-meter table-usage-${effectiveResetWindow}`}>
      <div className="card-usage-head">
        <span className="card-usage-value">
          <strong className={tone}>{remainingLabel}</strong>
          <span className="card-usage-label">
            {cardLabel && <span className="card-usage-name">{cardLabel}</span>}
            {cardLabelSuffix && <span>{cardLabelSuffix}</span>}
            <span className="card-usage-remaining">{t("usage.remaining")}</span>
          </span>
          {duration && <span className="usage-window-duration">{duration}</span>}
        </span>
      </div>
      <Progress percent={remaining} showInfo={false} size="small" strokeColor={usageStroke(remaining)} />
      <span className="usage-reset">{tableResetLabel(usageWindow.resetsAt, language, effectiveResetWindow, now)}</span>
      {failureNotice}
    </div>
  );
  return (
    <div className={`table-usage table-usage-${effectiveResetWindow}`}>
      <div className="table-usage-head">
        <strong className={tone}>{remainingLabel}</strong>
        <span>{t("usage.remaining")}</span>
        {duration && <span className="usage-window-duration">{duration}</span>}
        <UsageRefreshAge fetchedAt={fetchedAt} className="usage-recent-refresh" language={language} t={t} />
      </div>
      <Progress percent={remaining} showInfo={false} size="small" strokeColor={usageStroke(remaining)} />
      <span className="usage-reset">
        <span>{tableResetLabel(usageWindow.resetsAt, language, effectiveResetWindow, now)}</span>
      </span>
      {failureNotice}
    </div>
  );
}
