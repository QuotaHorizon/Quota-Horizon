import { Alert, Tag } from "antd";
import type { DesktopDiagnosticsEnvelope, DesktopStatusEnvelope, DesktopVaultMutationStatusEnvelope } from "../../../../capacity-preview/src/status";
import type { Language } from "../../i18n";
import { capacityAuthLabel, capacityPlanLabel, capacityStateLabel, safeSwitchPresentation } from "./statusCopy";
import styles from "./index.module.less";

export function CapacityDetails({ envelope, diagnostics, vault, language, onInspect }: {
  envelope: DesktopStatusEnvelope | null;
  diagnostics: DesktopDiagnosticsEnvelope | null;
  vault: DesktopVaultMutationStatusEnvelope | null;
  language: Language;
  onInspect: () => void;
}) {
  const zh = language === "zh";
  const status = envelope?.status;
  const availability = status?.dataStatus.availability === "complete" && status.resetCredits.summaryStatus === "unavailable"
    ? "partial" : status?.dataStatus.availability;
  const protection = safeSwitchPresentation(vault, language);
  const pending = vault?.catalog == null ? null
    : vault.catalog.pendingAccountOperationCount + vault.catalog.pendingKeyRotationCount;
  const observed = status?.quotaObservedAt ?? status?.capturedAt;
  const updated = observed && Number.isFinite(Date.parse(observed)) ? new Intl.DateTimeFormat(zh ? "zh-CN" : "en-US", {
    month: "short", day: "numeric", hour: "2-digit", minute: "2-digit", hourCycle: "h23",
  }).format(new Date(observed)) : "—";
  return <>
    <div className={styles.accountSummary}>
      <span>{zh ? "套餐" : "Plan"} · <strong>{status?.account?.planType ? capacityPlanLabel(status.account.planType, language) : "—"}</strong></span>
      <span>{capacityAuthLabel(status?.account?.authMode, language)}</span>
      <span>{zh ? "最近更新" : "Updated"} · {updated}</span>
    </div>
    <details className={styles.diagnosticsDisclosure} onToggle={(event) => { if (event.currentTarget.open) onInspect(); }}>
      <summary>{zh ? "运行状态与问题排查" : "Status and troubleshooting"}</summary>
      <div className={styles.detailsGrid}>
        <article className={styles.detailCard}>
          <div className={styles.detailTitle}><span>{zh ? "额度读取" : "Quota reading"}</span>
            <Tag color={availability === "complete" ? "green" : "orange"}>
              {capacityStateLabel(availability, language)}
            </Tag>
          </div>
          <dl>
            <div><dt>{zh ? "Codex 版本" : "Codex version"}</dt><dd>{diagnostics?.preview?.discovery.codexVersion ?? status?.codexVersion ?? "—"}</dd></div>
            <div><dt>{zh ? "兼容性" : "Compatibility"}</dt><dd>{capacityStateLabel(status?.dataStatus.compatibility, language)}</dd></div>
            <div><dt>{zh ? "重置卡读取" : "Reset cards"}</dt><dd>{capacityStateLabel(status?.resetCredits.summaryStatus, language)}</dd></div>
            <div><dt>{zh ? "本地历史" : "Local history"}</dt><dd>{capacityStateLabel(status?.account?.bindingStatus, language)}</dd></div>
          </dl>
        </article>
        <article className={styles.detailCard}>
          <div className={styles.detailTitle}><span>{zh ? "本地操作保护" : "Local operation protection"}</span></div>
          <Alert type={protection.kind} showIcon message={protection.title} description={protection.description} />
          {pending != null && (pending > 0 || (vault?.catalog?.needsReviewCount ?? 0) > 0) && <dl>
            <div><dt>{zh ? "待恢复的账户操作" : "Account operations awaiting recovery"}</dt><dd>{pending}</dd></div>
            <div><dt>{zh ? "需要复核" : "Needs review"}</dt><dd>{vault?.catalog?.needsReviewCount ?? "—"}</dd></div>
          </dl>}
        </article>
      </div>
      <details className={styles.technicalDetails}>
        <summary>{zh ? "技术详情（仅用于排查）" : "Technical details (troubleshooting only)"}</summary>
        {envelope?.issue?.code && <p>{zh ? "读取诊断代码" : "Read diagnostic code"}: <code>{envelope.issue.code}</code></p>}
        <p>{zh ? "操作保护诊断代码" : "Operation protection code"}: <code>{vault?.reasonCode ?? "—"}</code></p>
        <p>{zh ? "协调状态" : "Coordination status"}: {capacityStateLabel(vault?.coordination.lockState, language)}</p>
        <p>{zh ? "诊断读取" : "Diagnostic read"}: {capacityStateLabel(diagnostics?.status, language)}</p>
      </details>
    </details>
  </>;
}
