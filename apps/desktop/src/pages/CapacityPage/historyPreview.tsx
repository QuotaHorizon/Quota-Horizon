// Synthetic history fixture for browser-only visual previews.
import React from "react";
import { createRoot } from "react-dom/client";
import type { DesktopHistoryEnvelope } from "../../../../capacity-preview/src/status";
import { CapacityHistory } from "./CapacityHistory";
import { historyActivity } from "./historyActivity";
import "../../styles.css";
import styles from "./index.module.less";

const query = new URLSearchParams(location.search);
const dark = query.get("theme") === "dark";
const language = query.get("lang") === "en" ? "en" : "zh";
const scenario = query.get("scenario");
document.documentElement.dataset.theme = dark ? "dark" : "light";
const history: DesktopHistoryEnvelope = { schemaVersion: "1.0", status: "available", reasonCode: "history_available", points: [] };
for (let day = 1; day <= 10; day += 1) {
  if (day === 5) continue;
  for (let hour = 9; hour <= 18; hour += 1) {
    const capturedAt = new Date(2026, 8, day, hour).toISOString();
    const remaining = scenario === "flat" ? 28 : day < 7 ? 68 - day * 8 - (hour - 9) * .5 : 100 - (day - 7) * 7 - (hour - 9) * .6;
    history.points.push({ snapshotId: capturedAt, capturedAt, limitId: "codex:weekly", label: "Weekly", windowMinutes: 10080,
      usedPercent: 100 - remaining, remainingPercent: remaining,
      resetsAt: day < 7 ? "2026-09-07T00:00:00Z" : "2026-09-14T00:00:00Z", availability: "complete", compatibility: "tested" });
  }
}
if (scenario === "single") history.points = history.points.slice(-1);
history.overview = { sampleCount: history.points.length, dailyActivity: historyActivity(history) };

createRoot(document.getElementById("root")!).render(<React.StrictMode>
  <main style={{ maxWidth: 1000, margin: "24px auto", padding: "0 18px" }}>
    <p style={{ color: "var(--muted)", fontSize: 12 }}>{language === "zh" ? "合成数据预览" : "Synthetic data preview"}</p>
    <section className={styles.historyCard}>
      <div className={styles.sectionHeading}><h3>{language === "zh" ? "额度历史" : "Quota history"}</h3><small>{language === "zh" ? "按账户保存 · 后台自动更新" : "Per-account history · Background updates"}</small></div>
      <CapacityHistory history={history} language={language} dark={dark} />
    </section>
  </main>
</React.StrictMode>);
