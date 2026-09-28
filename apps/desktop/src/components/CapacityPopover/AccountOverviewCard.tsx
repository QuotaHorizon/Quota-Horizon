import { ChevronRight, TriangleAlert } from "lucide-react";
import { maskAccountEmail } from "../../utils/accountPrivacy";
import { accountUsageWindows } from "../../utils/accountUsageWindows";
import { quotaPercentLabel } from "../../utils/quotaPercent";
import type { AccountOverview } from "./accountOverview";
import { accountWeeklyResetDate, accountUsageFreshnessLabel, paceTone, targetTimeLabel } from "./presentation";
import styles from "./index.module.less";

export function AccountOverviewCard({ account, privacyMode, now, language, onSwitch }: {
  account: AccountOverview;
  privacyMode: boolean;
  now: number;
  language: "en" | "zh";
  onSwitch: (id: string) => void;
}) {
  const zh = language === "zh";
  const quota = accountUsageWindows(account.usage, account.plan);
  const plan = account.workPlan?.baseline?.comparison;
  const enabled = account.workPlan?.schedule?.enabled;
  const unreadable = account.usage.error === "account_record_unreadable";
  const loginRequired = /HTTP\s+(400|401|403)\b|请重新登录|登录已过期/i.test(account.usage.error ?? "");
  const margin = plan ? plan.actualRemainingPercent - plan.expectedRemainingPercent : 0;
  return (
    <article className={styles.accountCard} data-current={account.active} data-pace={paceTone(plan?.state)} aria-label={privacyMode ? maskAccountEmail(account.email) : account.email}>
      <div className={styles.accountCardHeader}>
        <div className={styles.accountIdentity}>
          <strong>{privacyMode ? maskAccountEmail(account.email) : account.email}</strong>
          <small>{account.plan || "ChatGPT"} · {accountUsageFreshnessLabel(account.usage.fetchedAt, Boolean(account.usage.error), now, language)}</small>
        </div>
        <div className={styles.accountCardQuota}>
          <span><small>{zh ? "周" : "Week"}</small><strong>{quotaPercentLabel(quota.weekly?.remainingPercent)}</strong></span>
          {quota.short && <span><small>5h</small><strong>{quotaPercentLabel(quota.short.remainingPercent)}</strong></span>}
        </div>
      </div>
      {enabled && plan ? (
        <div className={styles.accountCardPlan}>
          <span><small>{zh ? "停用前目标" : "Stop target"}</small><strong>{plan.expectedRemainingPercent.toFixed(1)}%</strong></span>
          <span className={styles.accountPlanMargin}><small>{margin >= 0 ? (zh ? "领先" : "Ahead") : (zh ? "超支" : "Over plan")}</small><strong>{Math.abs(margin).toFixed(1)}%</strong></span>
        </div>
      ) : <div className={styles.accountPlanUnavailable}>{enabled ? (zh ? "等待有效周额度 · 不借用其它账号数据" : "Waiting for this account's weekly quota") : (zh ? "工作计划未开启 · 可在“工作计划”统一设置" : "Plan off · configure the shared Work Plan")}</div>}
      <div className={styles.accountCardFooter}>
        <small title={enabled && plan ? targetTimeLabel(plan.targetAt, language) : undefined}>
          {zh ? "周重置" : "Weekly reset"} {accountWeeklyResetDate(quota.weekly?.resetsAt, language)}
        </small>
        {account.active ? <span className={styles.currentAccount}>{zh ? "使用中" : "In use"}</span> :
          <button type="button" disabled={unreadable} onClick={() => onSwitch(account.id)} aria-label={`${zh ? "预览切换账号" : "Review account switch"} ${privacyMode ? maskAccountEmail(account.email) : account.email}`}>
            {zh ? "切换使用" : "Switch"}<ChevronRight size={10} />
          </button>}
      </div>
      {account.usage.error && <div className={styles.accountCardWarning} role="status"><TriangleAlert size={11} />
        <span>{unreadable ? (zh ? "本地记录需修复，账号与缓存仍保留" : "Local record needs repair; account and cache retained") : loginRequired ? (zh ? "登录需处理，账号与上次额度仍保留" : "Sign-in needs attention; account and last quota retained") : (zh ? "刷新暂未成功，保留上次额度并稍后重试" : "Refresh failed; last quota retained, retrying later")}</span>
      </div>}
    </article>
  );
}
