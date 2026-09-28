import type { ReactNode } from "react";
import { ArrowRight, LogIn, RefreshCw } from "lucide-react";
import type { Language, Translate } from "../i18n";
import type { AccountDisplayMode } from "../hooks/useAccountDisplayMode";
import type { AccountRefreshProgress } from "../hooks/accountRefreshCoordinator";
import type {
  Account, AccountDetailsDraft, LocalProxyStatus, Provider,
  ResetCreditsLoadState,
} from "../types";
import { AccountTable } from "../components/accounts/AccountTable";

export function AccountsPage({
  active,
  accounts,
  providers,
  loading,
  busyAccountId,
  refreshingAccountIds,
  refreshingAll,
  refreshProgress,
  localProxy,
  proxyBusy,
  resetCredits,
  onAdd,
  onSwitch,
  onDeactivate,
  onRollbackLastChange,
  rollbackLastChangeBusy,
  onCopyAuthJson,
  onRefresh,
  onRefreshAll,
  onDelete,
  onConsumeQuotaMany,
  onDeleteMany,
  onEnableMany,
  onDisableMany,
  onAutoSwitchEnabledChange,
  autoSwitchBusyAccountId,
  onAutoSwitchPriorityChange,
  autoSwitchPriorityBusyAccountId,
  onAutoSwitchThresholdChange,
  autoSwitchThresholdBusyAccountId,
  onGlobalAutoSwitchThresholdChange,
  onSaveNote,
  onLoadAccountDetails,
  onLoadResetCredits,
  onUseResetCredit,
  resetCreditBusyAccountId,
  onOpenaiAuthAccountChange,
  onConcurrentRoutingChange,
  privacyMode,
  privacyModeLoading,
  onPrivacyModeChange,
  hideAccountNotes,
  showUsageNetworkErrors,
  displayMode,
  tokenUsageRefreshSeconds,
  proxyControls,
  language,
  t,
}: {
  active: boolean;
  accounts: Account[];
  providers: Provider[];
  loading: boolean;
  busyAccountId: string | null;
  refreshingAccountIds: string[];
  refreshingAll: boolean;
  refreshProgress: AccountRefreshProgress | null;
  localProxy: LocalProxyStatus | null;
  proxyBusy: boolean;
  resetCredits: Record<string, ResetCreditsLoadState>;
  onAdd: () => void;
  onSwitch: (id: string) => void;
  onDeactivate: (id: string) => void;
  onRollbackLastChange: () => void;
  rollbackLastChangeBusy: boolean;
  onCopyAuthJson: (id: string) => void;
  onRefresh: (id: string) => void;
  onRefreshAll: () => void;
  onDelete: (id: string) => void;
  onConsumeQuotaMany: (ids: string[]) => Promise<string[]>;
  onDeleteMany: (ids: string[]) => Promise<string[]>;
  onEnableMany: (ids: string[]) => Promise<string[]>;
  onDisableMany: (ids: string[]) => Promise<string[]>;
  onAutoSwitchEnabledChange: (id: string, enabled: boolean) => void;
  autoSwitchBusyAccountId: string | null;
  onAutoSwitchPriorityChange: (id: string, priority: number) => Promise<boolean>;
  autoSwitchPriorityBusyAccountId: string | null;
  onAutoSwitchThresholdChange: (id: string, threshold: number) => Promise<boolean>;
  autoSwitchThresholdBusyAccountId: string | null;
  onGlobalAutoSwitchThresholdChange: (threshold: number) => Promise<boolean>;
  onSaveNote: (id: string, details: AccountDetailsDraft) => Promise<boolean>;
  onLoadAccountDetails: (id: string) => Promise<Account | null>;
  onLoadResetCredits: (id: string, force?: boolean) => void;
  onUseResetCredit: (id: string) => void;
  resetCreditBusyAccountId: string | null;
  onOpenaiAuthAccountChange: (accountId: string | null) => void;
  onConcurrentRoutingChange: (enabled: boolean) => void;
  privacyMode: boolean;
  privacyModeLoading: boolean;
  onPrivacyModeChange: (enabled: boolean) => void;
  hideAccountNotes: boolean;
  showUsageNetworkErrors: boolean;
  displayMode: AccountDisplayMode;
  tokenUsageRefreshSeconds: number;
  proxyControls?: ReactNode;
  language: Language;
  t: Translate;
}) {
  const hotSwitchEnabled = Boolean(localProxy?.running);
  if (loading) {
    return (
      <div className="accounts-page">
        <div className="loading-state"><RefreshCw className="spin" />{t("accounts.loading")}</div>
      </div>
    );
  }
  if (!accounts.length) {
    return (
      <div className="accounts-page">
        {proxyControls && <div className="account-empty-proxy-toolbar">{proxyControls}</div>}
        <div className="empty-state">
          <div><LogIn size={28} /></div><h2>{t("accounts.empty.title")}</h2>
          <p>{t("accounts.empty.description")}</p>
          <button className="primary-button" onClick={onAdd}>
            {t("accounts.empty.addFirst")}<ArrowRight size={17} />
          </button>
        </div>
      </div>
    );
  }
  return (
    <div className="accounts-page">
      <AccountTable active={active} accounts={accounts} providers={providers} busyAccountId={busyAccountId}
        refreshingAccountIds={refreshingAccountIds} refreshingAll={refreshingAll}
        refreshProgress={refreshProgress}
        onSwitch={onSwitch} onDeactivate={onDeactivate}
        onRollbackLastChange={onRollbackLastChange} rollbackLastChangeBusy={rollbackLastChangeBusy}
        onCopyAuthJson={onCopyAuthJson} onRefresh={onRefresh} onRefreshAll={onRefreshAll}
        onDelete={onDelete}
        onConsumeQuotaMany={onConsumeQuotaMany} onDeleteMany={onDeleteMany}
        onEnableMany={onEnableMany} onDisableMany={onDisableMany}
        onAutoSwitchEnabledChange={onAutoSwitchEnabledChange} autoSwitchBusyAccountId={autoSwitchBusyAccountId}
        onAutoSwitchPriorityChange={onAutoSwitchPriorityChange}
        autoSwitchPriorityBusyAccountId={autoSwitchPriorityBusyAccountId}
        onAutoSwitchThresholdChange={onAutoSwitchThresholdChange}
        autoSwitchThresholdBusyAccountId={autoSwitchThresholdBusyAccountId}
        autoSwitchOnQuotaExhaustion={localProxy?.autoSwitchOnQuotaExhaustion ?? false}
        customAutoSwitchPriorityEnabled={localProxy?.customAutoSwitchPriorityEnabled ?? false}
        customAutoSwitchThresholdEnabled={localProxy?.customAutoSwitchThresholdEnabled ?? false}
        globalAutoSwitchThreshold={localProxy?.globalAutoSwitchThreshold ?? 0}
        onGlobalAutoSwitchThresholdChange={onGlobalAutoSwitchThresholdChange}
        onSaveNote={onSaveNote}
        onLoadAccountDetails={onLoadAccountDetails}
        resetCredits={resetCredits} onLoadResetCredits={onLoadResetCredits}
        onUseResetCredit={onUseResetCredit} resetCreditBusyAccountId={resetCreditBusyAccountId}
        hotSwitchEnabled={hotSwitchEnabled} privacyMode={privacyMode}
        privacyModeLoading={privacyModeLoading} onPrivacyModeChange={onPrivacyModeChange}
        hideAccountNotes={hideAccountNotes}
        concurrentAccountRoutingEnabled={localProxy?.concurrentAccountRoutingEnabled ?? false}
        concurrentAccountRoutingBusy={proxyBusy}
        onConcurrentAccountRoutingChange={onConcurrentRoutingChange}
        showUsageNetworkErrors={showUsageNetworkErrors} displayMode={displayMode}
        openaiAuthAccountId={localProxy?.openaiAuthAccountId ?? null} openaiAuthBusy={proxyBusy}
        onOpenaiAuthAccountChange={onOpenaiAuthAccountChange}
        tokenUsageRefreshSeconds={tokenUsageRefreshSeconds}
        proxyControls={proxyControls}
        language={language} t={t} />
    </div>
  );
}
