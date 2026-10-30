// Synthetic history fixture for browser-only visual previews.
import React from "react";
import { createRoot } from "react-dom/client";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import type { DesktopHistoryEnvelope } from "../../../../capacity-preview/src/status";
import { historyActivity } from "./historyActivity";
import "../../styles.css";

// Keep the native backend and the user's history outside this documentation entry.
if ("__TAURI_INTERNALS__" in window) throw new Error("History preview is browser-only");

const query = new URLSearchParams(location.search);
const dark = query.get("theme") === "dark";
const language = query.get("lang") === "en" ? "en" : "zh";
const scenario = query.get("scenario");
document.documentElement.lang = language === "zh" ? "zh-CN" : "en";
document.documentElement.dataset.theme = dark ? "dark" : "light";
const contextId = "synthetic-history";
const history: DesktopHistoryEnvelope = { schemaVersion: "1.0", historyContextId: contextId,
  status: "available", reasonCode: "history_available", points: [] };
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

// Import the real panel after installing an isolated, read-only synthetic IPC.
// Unknown commands fail instead of reaching a native service or a data store.
mockIPC((command) => {
  if (command === "capacity_get_history") return history;
  throw new Error(`Unsupported history preview command: ${command}`);
});
const root = createRoot(document.getElementById("root")!);
void import("./CapacityHistoryPanel").then(({ CapacityHistoryPanel }) => {
  root.render(<React.StrictMode>
    <main style={{ maxWidth: 1000, margin: "24px auto", padding: "0 18px" }}>
      <p style={{ color: "var(--muted)", fontSize: 12 }}>{language === "zh" ? "合成数据预览" : "Synthetic data preview"}</p>
      <CapacityHistoryPanel contextId={contextId} limitId="codex:weekly" sequence={1}
        language={language} dark={dark} authorizing={false} />
    </main>
  </React.StrictMode>);
});
if (import.meta.hot) import.meta.hot.dispose(() => { root.unmount(); clearMocks(); });
