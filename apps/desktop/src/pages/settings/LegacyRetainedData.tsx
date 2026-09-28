import { Tag } from "antd";
import type { Language } from "../../i18n";
import type { LegacyMigrationDryRunReport } from "../../types";
import { retainedDataCopy, retainedDataViews } from "./retainedViewerData";
import styles from "./LegacyMigrationCard.module.less";

export function LegacyRetainedData({ report, language }: { report: LegacyMigrationDryRunReport; language: Language }) {
  const copy = retainedDataCopy[language];
  return <details className={styles.retainedData}>
    <summary>{copy.title}</summary>
    <p>{copy.boundary}</p>
    {retainedDataViews(report, language).map((item) => <article key={item.kind}>
      <div><strong>{item.title}</strong><Tag color={item.status === "review" ? "gold" : "default"}>{item.statusLabel}</Tag></div>
      <p>{item.detail}</p><small>{item.next}</small>
    </article>)}
  </details>;
}
