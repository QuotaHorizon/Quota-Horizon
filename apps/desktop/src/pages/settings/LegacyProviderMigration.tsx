import { Alert, Button, Input, Tag } from "antd";
import { Cable, RotateCcw } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { confirmLegacyQuotaViewerProviderImport, getLatestLegacyProviderImport,
  prepareLegacyQuotaViewerProviderImport, rollbackLegacyQuotaViewerProviderImport } from "../../api/backend";
import type { Language } from "../../i18n";
import type { LegacyProviderImportOperation, LegacyProviderImportPreview, LegacyProviderImportRow } from "../../types";
import { providerImportReason, providerImportReasoning, providerMigrationCopy } from "./legacyProviderMigrationCopy";
import styles from "./LegacyMigrationCard.module.less";

export function ProviderImportRows({ rows, language, disabled, onSelect }: {
  rows: LegacyProviderImportRow[]; language: Language; disabled: boolean;
  onSelect: (row: LegacyProviderImportRow) => void;
}) {
  const copy = providerMigrationCopy[language];
  return <div className={styles.providerRows}>{rows.map((row) => {
    const reason = providerImportReason(row.reasonCode, language);
    const eligible = row.disposition === "import" && Boolean(row.confirmToken);
    return <article key={row.sourceOrdinal} className={styles.providerRow}>
      <div className={styles.providerHeading}>
        <strong>{row.label}</strong>
        <Tag color={eligible ? "green" : "gold"}>{eligible ? copy.import
          : row.disposition === "keep_existing" ? copy.keep_existing : copy.reviewOnly}</Tag>
      </div>
      {row.endpoint && <p><span>{copy.endpoint}</span>{row.endpoint}</p>}
      {row.model && <p><span>{copy.model}</span>{row.model}</p>}
      {row.apiFormat && <p><span>{copy.protocol}</span>{row.apiFormat === "openaiResponses"
        ? "Responses API" : row.apiFormat === "openaiChat" ? "Chat Completions" : copy.reviewOnly}</p>}
      {row.reasoningEffort && <p><span>{copy.reasoning}</span>{providerImportReasoning(row.reasoningEffort, language)}</p>}
      {typeof row.contextWindow === "number" && Number.isFinite(row.contextWindow) && row.contextWindow > 0
        && <p><span>{copy.context}</span>{row.contextWindow.toLocaleString()} tokens</p>}
      {reason && <small>{reason}</small>}
      {eligible && <Button size="small" disabled={disabled} onClick={() => onSelect(row)}>{copy.select}</Button>}
    </article>;
  })}</div>;
}

export function LegacyProviderMigration({ sourcePath, language, disabled, onBusy }: {
  sourcePath: string | null; language: Language; disabled: boolean; onBusy: (busy: boolean) => void;
}) {
  const copy = providerMigrationCopy[language];
  const [preview, setPreview] = useState<LegacyProviderImportPreview | null>(null);
  const [selected, setSelected] = useState<LegacyProviderImportRow | null>(null);
  const [typed, setTyped] = useState("");
  const [rollbackTyped, setRollbackTyped] = useState("");
  const [operation, setOperation] = useState<LegacyProviderImportOperation | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const mounted = useRef(true);
  const pending = useRef(false);
  const operationRevision = useRef(0);

  useEffect(() => {
    mounted.current = true;
    const revision = operationRevision.current;
    void getLatestLegacyProviderImport().then((value) => {
      if (mounted.current && revision === operationRevision.current) setOperation(value);
    }).catch(() => { /* Explicit review retries and reports the read failure. */ });
    return () => { mounted.current = false; };
  }, []);

  const run = async (action: () => Promise<void>) => {
    if (pending.current || disabled) return;
    pending.current = true;
    operationRevision.current += 1;
    setBusy(true); onBusy(true); setError(null);
    try { await action(); }
    catch (reason) {
      if (mounted.current) setError(reason instanceof Error ? reason.message : String(reason));
      try {
        const latest = await getLatestLegacyProviderImport();
        if (mounted.current) setOperation(latest);
      } catch { /* Preserve the original actionable failure. */ }
    } finally {
      pending.current = false;
      if (mounted.current) setBusy(false);
      onBusy(false);
    }
  };
  const review = () => run(async () => {
    setSelected(null); setTyped("");
    const [next, latest] = await Promise.all([
      prepareLegacyQuotaViewerProviderImport(sourcePath), getLatestLegacyProviderImport(),
    ]);
    if (mounted.current) { setPreview(next); setOperation(latest); }
  });
  const confirm = () => run(async () => {
    if (!selected?.confirmToken || typed !== "IMPORT API") return;
    let result: LegacyProviderImportOperation;
    try { result = await confirmLegacyQuotaViewerProviderImport(selected.confirmToken, typed); }
    catch (reason) {
      // A native confirmation token is single-use even if the shared lock or
      // source check refuses the import. Do not offer a dead confirmation again.
      if (mounted.current) { setPreview(null); setSelected(null); setTyped(""); }
      throw reason;
    }
    if (!mounted.current) return;
    setOperation(result); setSelected(null); setTyped("");
    // Other rows retain their independent, expiring native confirmation tokens.
    setPreview((value) => value ? { ...value, providers: value.providers.map((row) =>
      row.confirmToken === selected.confirmToken ? { ...row, disposition: "keep_existing",
        confirmToken: null, reasonCode: "legacy_provider_existing" } : row) } : value);
  });
  const rollback = () => run(async () => {
    if (!operation?.operationId || rollbackTyped !== "ROLLBACK API") return;
    const result = await rollbackLegacyQuotaViewerProviderImport(operation.operationId, rollbackTyped);
    if (mounted.current) { setOperation(result); setPreview(null); setSelected(null); setTyped(""); setRollbackTyped(""); }
  });

  return <section className={styles.accountStage}>
    <div className={styles.stageHeading}>
      <div><h4><Cable size={15} />{copy.title}</h4><p>{copy.description}</p></div>
      <Button size="small" disabled={disabled || busy} onClick={() => void review()}>
        {preview ? copy.reviewAgain : copy.review}</Button>
    </div>
    {busy && <Alert type="info" showIcon message={copy.busy} />}
    {error && <Alert type="error" showIcon message={copy.failed} description={error} />}
    {preview && (preview.providers.length === 0 ? <p>{copy.empty}</p> : <>
      <ProviderImportRows rows={preview.providers} language={language} disabled={disabled || busy}
        onSelect={(row) => { setSelected(row); setTyped(""); }} />
      <p className={styles.spaceLine}>{copy.connectionScope}</p>
    </>)}
    {selected && <section className={styles.applyPanel}>
      <h4>{copy.selected} · {selected.label}</h4>
      <Alert type="warning" showIcon message={copy.safety} />
      <div className={styles.confirmRow}>
        <Input aria-label={copy.selected} value={typed} placeholder="IMPORT API" disabled={busy || disabled}
          onChange={(event) => setTyped(event.target.value)} />
        <Button type="primary" disabled={disabled || busy || typed !== "IMPORT API"}
          onClick={() => void confirm()}>{copy.confirm}</Button>
      </div>
      <Button type="text" disabled={disabled || busy} onClick={() => setSelected(null)}>{copy.cancel}</Button>
    </section>}
    {operation?.status === "committed" && <Alert type="success" showIcon message={copy.committed} description={operation.label} />}
    {operation?.status === "rolled_back" && <Alert type="success" showIcon message={copy.rolledBack} description={operation.label} />}
    {operation?.recoveryRequired && <Alert type="warning" showIcon message={copy.unfinished} />}
    {operation?.rollbackAvailable && <section className={styles.rollbackPanel}>
      <h4>{copy.rollback} · {operation.label}</h4><p>{copy.rollbackHint}</p>
      <div className={styles.confirmRow}>
        <Input aria-label={copy.rollback} value={rollbackTyped} placeholder="ROLLBACK API" disabled={busy || disabled}
          onChange={(event) => setRollbackTyped(event.target.value)} />
        <Button danger icon={<RotateCcw size={14} />} disabled={disabled || busy || rollbackTyped !== "ROLLBACK API"}
          onClick={() => void rollback()}>{copy.rollbackAction}</Button>
      </div>
    </section>}
    {operation?.status === "committed" && !operation.rollbackAvailable && <p>{copy.notReversible}</p>}
  </section>;
}
