import { Alert, Button, Input, Modal, Tag } from "antd";
import { FolderSearch, Import, RotateCcw, Search, ShieldCheck, Users } from "lucide-react";
import { useMemo, useState } from "react";
import {
  chooseLegacyQuotaViewerDirectory,
  confirmLegacyQuotaViewerAccountImport,
  confirmLegacyQuotaViewerImport,
  getLatestLegacyAccountMigrationOperation,
  getLatestLegacyMigrationOperation,
  inspectLegacyQuotaViewerMigration,
  prepareLegacyQuotaViewerAccountImport,
  prepareLegacyQuotaViewerImport,
  rollbackLegacyQuotaViewerAccountImport,
  rollbackLegacyQuotaViewerImport,
} from "../../api/backend";
import type {
  LegacyAccountMigrationOperationView,
  LegacyAccountMigrationPreview,
  LegacyAccountMigrationResult,
  LegacyMigrationApplyPreview,
  LegacyMigrationApplyResult,
  LegacyMigrationDryRunReport,
  LegacyMigrationOperationView,
} from "../../types";
import type { SettingsPageProps } from "./types";
import {
  formatMigrationBytes,
  legacyArtifactLabels,
  legacyAccountAuthModeLabels,
  legacyAccountDispositionLabels,
  legacyConflictLabels,
  legacyDispositionLabels,
  legacyMigrationCopy,
  legacyImportedLanguageLabel,
  legacyOptionalStateLabel,
  legacyStateLabels,
  legacyTargetLabels,
  migrationDispositionColor,
  migrationAccountDispositionColor,
  migrationArtifactStateColor,
  migrationPlanCounts,
  migrationStatusColor,
  selectedMigrationFolderLabel,
} from "./legacyMigration";
import styles from "./LegacyMigrationCard.module.less";
import { LegacyProviderMigration } from "./LegacyProviderMigration";
import { LegacyRetainedData } from "./LegacyRetainedData";
import { providerMigrationCopy } from "./legacyProviderMigrationCopy";

function interpolate(value: string, variables: Record<string, string | number>) {
  return Object.entries(variables).reduce(
    (text, [key, replacement]) => text.split(`{${key}}`).join(String(replacement)),
    value,
  );
}

export function LegacyMigrationCard({ settings }: { settings: SettingsPageProps }) {
  const { language } = settings;
  const copy = legacyMigrationCopy[language];
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [busyMessage, setBusyMessage] = useState<string | null>(null);
  const [report, setReport] = useState<LegacyMigrationDryRunReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [sourcePath, setSourcePath] = useState<string | null>(null);
  const [sourceName, setSourceName] = useState<string | null>(null);
  const [applyPreview, setApplyPreview] = useState<LegacyMigrationApplyPreview | null>(null);
  const [applyResult, setApplyResult] = useState<LegacyMigrationApplyResult | null>(null);
  const [operation, setOperation] = useState<LegacyMigrationOperationView | null>(null);
  const [typedConfirmation, setTypedConfirmation] = useState("");
  const [typedRollback, setTypedRollback] = useState("");
  const [accountPreview, setAccountPreview] =
    useState<LegacyAccountMigrationPreview | null>(null);
  const [accountResult, setAccountResult] = useState<LegacyAccountMigrationResult | null>(null);
  const [accountOperation, setAccountOperation] =
    useState<LegacyAccountMigrationOperationView | null>(null);
  const [typedAccountConfirmation, setTypedAccountConfirmation] = useState("");
  const [typedAccountRollback, setTypedAccountRollback] = useState("");
  const counts = useMemo(() => report ? migrationPlanCounts(report) : null, [report]);
  const sourceLabel = sourceName
    ? interpolate(copy.selectedSource, { name: sourceName })
    : copy.defaultSource;

  const inspect = async (nextSourcePath: string | null, nextSourceName: string | null) => {
    setBusy(true);
    setBusyMessage(copy.inspecting);
    setError(null);
    setReport(null);
    setApplyPreview(null);
    setApplyResult(null);
    setTypedConfirmation("");
    setTypedRollback("");
    setAccountPreview(null);
    setAccountResult(null);
    setTypedAccountConfirmation("");
    setTypedAccountRollback("");
    setSourcePath(nextSourcePath);
    setSourceName(nextSourceName);
    setOpen(true);
    try {
      const [nextReport, latestOperation, latestAccountOperation] = await Promise.all([
        inspectLegacyQuotaViewerMigration(nextSourcePath),
        getLatestLegacyMigrationOperation(),
        getLatestLegacyAccountMigrationOperation(),
      ]);
      setReport(nextReport);
      setOperation(latestOperation);
      setAccountOperation(latestAccountOperation);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
      setBusyMessage(null);
    }
  };

  const refreshOperation = async () => {
    setOperation(await getLatestLegacyMigrationOperation());
  };

  const refreshAccountOperation = async () => {
    setAccountOperation(await getLatestLegacyAccountMigrationOperation());
  };

  const prepareImport = async () => {
    setBusy(true);
    setBusyMessage(copy.preparingImport);
    setError(null);
    setApplyResult(null);
    setTypedConfirmation("");
    try {
      setApplyPreview(await prepareLegacyQuotaViewerImport(sourcePath));
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
      setBusyMessage(null);
    }
  };

  const confirmImport = async () => {
    if (!applyPreview?.confirmToken) return;
    setBusy(true);
    setBusyMessage(copy.importing);
    setError(null);
    try {
      const result = await confirmLegacyQuotaViewerImport(
        applyPreview.confirmToken,
        typedConfirmation,
      );
      setApplyResult(result);
      setApplyPreview(null);
      setTypedConfirmation("");
      await refreshOperation();
      if (result.language === "en" || result.language === "zh") {
        settings.onLanguageChange(result.language);
      }
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
      setBusyMessage(null);
    }
  };

  const rollbackImport = async () => {
    if (!operation?.operationId) return;
    setBusy(true);
    setBusyMessage(copy.rollingBack);
    setError(null);
    try {
      const result = await rollbackLegacyQuotaViewerImport(operation.operationId, typedRollback);
      setApplyResult(result);
      setTypedRollback("");
      await refreshOperation();
      if (result.language === "en" || result.language === "zh") {
        settings.onLanguageChange(result.language);
      }
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
      setBusyMessage(null);
    }
  };

  const prepareAccountImport = async () => {
    setBusy(true);
    setBusyMessage(copy.preparingAccounts);
    setError(null);
    setAccountResult(null);
    setTypedAccountConfirmation("");
    try {
      setAccountPreview(await prepareLegacyQuotaViewerAccountImport(sourcePath));
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
      setBusyMessage(null);
    }
  };

  const confirmAccountImport = async () => {
    if (!accountPreview?.confirmToken) return;
    setBusy(true);
    setBusyMessage(copy.importingAccounts);
    setError(null);
    try {
      const result = await confirmLegacyQuotaViewerAccountImport(
        accountPreview.confirmToken,
        typedAccountConfirmation,
      );
      setAccountResult(result);
      setAccountPreview(null);
      setTypedAccountConfirmation("");
      await refreshAccountOperation();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
      setBusyMessage(null);
    }
  };

  const rollbackAccountImport = async () => {
    if (!accountOperation?.operationId) return;
    setBusy(true);
    setBusyMessage(copy.rollingBackAccounts);
    setError(null);
    try {
      const result = await rollbackLegacyQuotaViewerAccountImport(
        accountOperation.operationId,
        typedAccountRollback,
      );
      setAccountResult(result);
      setTypedAccountRollback("");
      await refreshAccountOperation();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
      setBusyMessage(null);
    }
  };

  const choose = async () => {
    const selected = await chooseLegacyQuotaViewerDirectory();
    if (!selected) return;
    await inspect(selected, selectedMigrationFolderLabel(selected));
  };

  const statusText = report ? copy[report.status] : copy.failed;
  const hasAccountRecords = report?.inventory.some(
    (artifact) => artifact.kind === "account_records" && artifact.state === "present",
  ) ?? false;
  return (
    <>
      <section className="settings-card">
        <div className="settings-icon"><Import size={23} /></div>
        <div className="settings-card-content">
          <div className="settings-card-copy">
            <h3>{copy.title}</h3>
            <p>{copy.description}</p>
          </div>
          <div className={styles.actions}>
            <Button size="small" type="primary" icon={<Search size={14} />} loading={busy}
              onClick={() => void inspect(null, null)}>
              {copy.inspectDefault}
            </Button>
            <Button size="small" icon={<FolderSearch size={14} />} disabled={busy}
              onClick={() => void choose()}>
              {copy.chooseBackup}
            </Button>
          </div>
        </div>
      </section>
      <Modal className={styles.modal} open={open} width={900} title={copy.modalTitle}
        maskClosable={!busy} keyboard={!busy} closable={!busy} onCancel={() => setOpen(false)}
        footer={[
          <Button key="again" icon={<Search size={14} />} disabled={busy}
            onClick={() => void inspect(sourcePath, sourceName)}>{copy.inspectAgain}</Button>,
          <Button key="close" type="primary" disabled={busy}
            onClick={() => setOpen(false)}>{copy.close}</Button>,
        ]}>
        <div className={styles.sourceRow}>
          <strong>{sourceLabel}</strong>
          <span><ShieldCheck size={14} />{copy.readOnly}</span>
        </div>
        {busy && <Alert type="info" showIcon message={busyMessage ?? copy.inspecting} />}
        {error && <Alert type="error" showIcon message={copy.operationFailed} description={error} />}
        {report && counts && (
          <div className={styles.report}>
            <div className={styles.statusRow}>
              <Tag color={migrationStatusColor(report.status)}>{statusText}</Tag>
              <span>{interpolate(copy.schema, { version: report.schemaVersion })}</span>
              <span>{copy.format}: {report.source.format ?? "—"}</span>
              <span>{legacyDispositionLabels[language].import} {counts.import}</span>
              <span>{legacyDispositionLabels[language].review} {counts.review}</span>
              {counts.blocked > 0 && <span>{legacyDispositionLabels[language].blocked} {counts.blocked}</span>}
            </div>
            <p className={styles.spaceLine}>{interpolate(copy.sourceSize, {
              source: formatMigrationBytes(report.spaceEstimate.sourceBytes),
              minimum: formatMigrationBytes(report.spaceEstimate.minimumFreeBytes),
            })}</p>
            <h4>{copy.inventory}</h4>
            <div className={styles.grid}>
              {report.inventory.map((artifact) => (
                <div className={styles.item} key={artifact.kind}>
                  <div><strong>{legacyArtifactLabels[language][artifact.kind]}</strong>
                    <Tag color={migrationArtifactStateColor(artifact.state)}>
                      {legacyStateLabels[language][artifact.state]}
                    </Tag></div>
                  <small>{interpolate(copy.filesRecords, {
                    files: artifact.fileCount,
                    records: artifact.recordCount,
                    size: formatMigrationBytes(artifact.sourceBytes),
                  })}</small>
                </div>
              ))}
            </div>
            <h4>{copy.plan}</h4>
            <div className={styles.plan}>
              {report.plan.map((action) => (
                <div className={styles.planRow} key={action.target}>
                  <strong>{legacyTargetLabels[language][action.target]}</strong>
                  <span>{interpolate(copy.sourceCount, { source: action.sourceItemCount })}</span>
                  <span>{interpolate(copy.targetCount, { target: action.expectedTargetCount })}</span>
                  <Tag color={migrationDispositionColor(action.disposition)}>
                    {legacyDispositionLabels[language][action.disposition]}
                  </Tag>
                </div>
              ))}
            </div>
            <h4>{copy.conflicts}</h4>
            {report.conflicts.length === 0
              ? <p className={styles.noConflict}>{copy.noConflicts}</p>
              : <div className={styles.conflicts}>{report.conflicts.map((conflict) => (
                <Tag color="gold" key={`${conflict.kind}-${conflict.reasonCode}`}>
                  {legacyConflictLabels[language][conflict.kind]} × {conflict.count}
                </Tag>
              ))}</div>}
            <Alert type={report.status === "ready" ? "success" : report.status === "failed" ? "error" : "warning"} showIcon
              message={statusText} description={copy.next} />
            <LegacyRetainedData report={report} language={language} />
            {(report.status === "ready" || report.status === "partial") && counts.import > 0 && !applyPreview && (
              <div className={styles.applyActions}>
                <Button type="primary" icon={<Import size={14} />} loading={busy}
                  onClick={() => void prepareImport()}>{copy.prepareImport}</Button>
              </div>
            )}
            {applyPreview?.status === "already_applied" && (
              <Alert type="success" showIcon message={copy.alreadyApplied} />
            )}
            {applyPreview?.status === "confirmation_required" && (
              <section className={styles.applyPanel}>
                <h4>{copy.confirmationTitle}</h4>
                <div className={styles.summaryGrid}>
                  <span><small>{copy.refreshMode}</small><strong>{applyPreview.imported.autoRefreshEnabled
                    ? interpolate(copy.minutes, { minutes: (applyPreview.imported.refreshIntervalSeconds ?? 300) / 60 })
                    : applyPreview.imported.autoRefreshEnabled === false ? copy.manual : "—"}</strong></span>
                  <span><small>{copy.launchMode}</small><strong>{legacyOptionalStateLabel(
                    applyPreview.imported.launchAtLogin, copy.enabled, copy.disabled,
                  )}</strong></span>
                  <span><small>{copy.languageMode}</small><strong>{legacyImportedLanguageLabel(
                    applyPreview.imported.language,
                  )}</strong></span>
                  <span><small>{copy.workPlanMode}</small><strong>{applyPreview.imported.workPlanEnabled === null
                    ? "—"
                    : interpolate(copy.offPeriods, {
                      state: legacyOptionalStateLabel(
                        applyPreview.imported.workPlanEnabled, copy.enabled, copy.disabled,
                      ),
                      count: applyPreview.imported.offPeriodCount,
                    })}</strong></span>
                  <span className={styles.summaryWide}><small>{copy.statusPolicy}</small><strong>{copy.dynamicStatus}</strong></span>
                </div>
                <div className={styles.targetGroups}>
                  <span><b>{copy.changedTargets}</b>{applyPreview.changedTargets.map((target) => (
                    <Tag color="green" key={target}>{legacyTargetLabels[language][target]}</Tag>
                  ))}</span>
                  <span><b>{copy.reviewExcluded}</b>{applyPreview.reviewTargetsExcluded.map((target) => (
                    <Tag key={target}>{legacyTargetLabels[language][target]}</Tag>
                  ))}</span>
                </div>
                <Alert type="warning" showIcon message={copy.confirmationHint} />
                <div className={styles.confirmRow}>
                  <Input value={typedConfirmation} disabled={busy} placeholder="IMPORT"
                    onChange={(event) => setTypedConfirmation(event.target.value)} />
                  <Button type="primary" danger loading={busy}
                    disabled={typedConfirmation !== applyPreview.typedConfirmation}
                    onClick={() => void confirmImport()}>{copy.confirmImport}</Button>
                </div>
              </section>
            )}
            {applyResult && (
              <Alert type="success" showIcon
                message={applyResult.status === "rolled_back" ? copy.rolledBack : copy.importApplied}
                description={applyResult.rollbackAvailable ? copy.rollbackAvailable : undefined} />
            )}
            {operation?.recoveryRequired && (
              <Alert type="error" showIcon message={copy.recoveryRequired} />
            )}
            {operation?.rollbackAvailable && operation.operationId && (
              <section className={styles.rollbackPanel}>
                <h4>{copy.rollbackTitle}</h4>
                <p>{copy.rollbackHint}</p>
                <div className={styles.confirmRow}>
                  <Input value={typedRollback} disabled={busy} placeholder="ROLLBACK"
                    onChange={(event) => setTypedRollback(event.target.value)} />
                  <Button danger icon={<RotateCcw size={14} />} loading={busy}
                    disabled={typedRollback !== operation.typedRollback}
                    onClick={() => void rollbackImport()}>{copy.confirmRollback}</Button>
                </div>
              </section>
            )}
            {hasAccountRecords && (
              <section className={styles.accountStage}>
                <div className={styles.stageHeading}>
                  <div>
                    <h4><Users size={15} />{copy.accountTitle}</h4>
                    <p>{copy.accountDescription}</p>
                  </div>
                  <Button size="small" icon={<Users size={14} />} disabled={busy}
                    onClick={() => void prepareAccountImport()}>
                    {accountPreview ? copy.reviewAccountsAgain : copy.reviewAccounts}
                  </Button>
                </div>
                {accountPreview && (
                  <>
                    <div className={styles.accountCounts}>
                      <Tag color="green">{copy.accountImportCount} {accountPreview.importCount}</Tag>
                      <Tag>{copy.accountExistingCount} {accountPreview.alreadyPresentCount}</Tag>
                      {accountPreview.conflictCount > 0 && (
                        <Tag color="gold">{copy.accountConflictCount} {accountPreview.conflictCount}</Tag>
                      )}
                      {accountPreview.unsupportedCount > 0 && (
                        <Tag color="red">{copy.accountUnsupportedCount} {accountPreview.unsupportedCount}</Tag>
                      )}
                    </div>
                    <div className={styles.accountRows}>
                      {accountPreview.accounts.map((account) => (
                        <div className={styles.accountRow} key={account.sourceOrdinal}>
                          <span className={styles.accountIdentity}>
                            <strong>{account.label}</strong>
                            <small>#{account.sourceOrdinal} · {legacyAccountAuthModeLabels[language][account.authMode]}</small>
                          </span>
                          {account.preferred && <Tag color="blue">{copy.accountPreferred}</Tag>}
                          <Tag color={migrationAccountDispositionColor(account.disposition)}>
                            {legacyAccountDispositionLabels[language][account.disposition]}
                          </Tag>
                        </div>
                      ))}
                    </div>
                    <Alert type={accountPreview.status === "already_applied" ? "success" : "warning"}
                      showIcon message={accountPreview.status === "already_applied"
                        ? copy.accountsAlreadyApplied
                        : accountPreview.status === "review_only"
                          ? copy.accountsReviewOnly
                          : copy.accountSafetySummary}
                      description={copy.accountSafetyDetail} />
                    {accountPreview.status === "confirmation_required" && accountPreview.confirmToken && (
                      <section className={styles.accountConfirmPanel}>
                        <Alert type="warning" showIcon message={copy.accountConfirmationHint} />
                        <div className={styles.confirmRow}>
                          <Input value={typedAccountConfirmation} disabled={busy}
                            placeholder="IMPORT ACCOUNTS"
                            onChange={(event) => setTypedAccountConfirmation(event.target.value)} />
                          <Button type="primary" danger loading={busy}
                            disabled={typedAccountConfirmation !== accountPreview.typedConfirmation}
                            onClick={() => void confirmAccountImport()}>{copy.confirmAccountImport}</Button>
                        </div>
                      </section>
                    )}
                  </>
                )}
                {accountResult && (
                  <Alert type="success" showIcon
                    message={accountResult.status === "rolled_back"
                      ? copy.accountsRolledBack
                      : interpolate(copy.accountsImported, { count: accountResult.importedCount })}
                    description={accountResult.rollbackAvailable
                      ? copy.accountRollbackAvailable
                      : copy.accountCurrentLoginPreserved} />
                )}
                {accountOperation?.recoveryRequired && (
                  <Alert type="error" showIcon message={copy.accountRecoveryRequired} />
                )}
                {accountOperation?.rollbackAvailable && accountOperation.operationId && (
                  <section className={styles.rollbackPanel}>
                    <h4>{copy.accountRollbackTitle}</h4>
                    <p>{copy.accountRollbackHint}</p>
                    <div className={styles.confirmRow}>
                      <Input value={typedAccountRollback} disabled={busy}
                        placeholder="ROLLBACK ACCOUNTS"
                        onChange={(event) => setTypedAccountRollback(event.target.value)} />
                      <Button danger icon={<RotateCcw size={14} />} loading={busy}
                        disabled={typedAccountRollback !== accountOperation.typedRollback}
                        onClick={() => void rollbackAccountImport()}>{copy.confirmAccountRollback}</Button>
                    </div>
                  </section>
                )}
              </section>
            )}
            {hasAccountRecords && <LegacyProviderMigration key={sourcePath ?? "default"}
              sourcePath={sourcePath} language={language} disabled={busy}
              onBusy={(value) => { setBusy(value); setBusyMessage(value ? providerMigrationCopy[language].busy : null); }} />}
          </div>
        )}
      </Modal>
    </>
  );
}
