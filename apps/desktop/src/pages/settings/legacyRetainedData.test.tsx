import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { LegacyMigrationDryRunReport } from "../../types";
import { LegacyRetainedData } from "./LegacyRetainedData";
import { retainedDataViews } from "./retainedViewerData";

function report(): LegacyMigrationDryRunReport {
  return { schemaVersion: "1.0", status: "ready", reasonCodes: [],
    source: { product: "codex_quota_viewer", format: null, baselineCommit: "fixture", readOnly: true },
    inventory: ["quota_cache", "provider_mode", "restore_points"].map((kind) => ({
      kind, state: "present", fileCount: 1, recordCount: 2, sourceBytes: 128, reasonCodes: ["internal_fixture_reason"],
    })) as LegacyMigrationDryRunReport["inventory"], plan: [], conflicts: [],
    spaceEstimate: { sourceBytes: 384, minimumFreeBytes: 1024, status: "estimated" } };
}

describe("retained Viewer state is not advertised as migrated", () => {
  it("explains the real boundary without performing any action", () => {
    const views = retainedDataViews(report(), "zh");
    expect(views.map((view) => view.status)).toEqual(["retained", "retained", "retained"]);
    expect(views[0].detail).toContain("2 条旧快照");
    expect(views[0].next).toContain("旧快照保留在 Viewer");
    expect(views[1].detail).toContain("没有自动启用");
    expect(views[2].detail).toContain("尚未逐字节核验");
    expect(views[2].next).toContain("完整旧记录迁移仍待完成");
  });

  it("makes a missing mode recovery dependency actionable even if the mode record is readable", () => {
    const input = report();
    input.conflicts = [{ kind: "provider_restore_point_missing", count: 1, reasonCode: "internal_failure" }];
    const view = retainedDataViews(input, "zh")[1];
    expect(view.status).toBe("review");
    expect(view.detail).toContain("恢复记录缺失");
    expect(view.next).toContain("请先从 Viewer 或完整备份找回对应恢复记录");
  });

  it("distinguishes missing from unreadable or incomplete data", () => {
    const missing = report();
    missing.inventory.forEach((item) => { item.state = "missing"; item.recordCount = 0; });
    expect(retainedDataViews(missing, "zh").every((view) => view.status === "missing")).toBe(true);
    for (const state of ["invalid", "unsafe", "limit_exceeded", "future_internal_state"]) {
      const input = report();
      input.inventory.forEach((item) => { item.state = state as typeof item.state; });
      expect(retainedDataViews(input, "zh").every((view) => view.status === "review")).toBe(true);
    }
    const absent = report(); absent.inventory = [];
    expect(retainedDataViews(absent, "zh").every((view) => view.status === "review")).toBe(true);
    const unknownCount = report(); unknownCount.inventory[0].recordCount = Number.NaN;
    expect(retainedDataViews(unknownCount, "zh")[0].status).toBe("review");
  });

  it("keeps ordinary summaries collapsed, bilingual, and free of internal identifiers", () => {
    for (const language of ["zh", "en"] as const) {
      const markup = renderToStaticMarkup(<LegacyRetainedData report={report()} language={language} />);
      expect(markup).not.toMatch(/<details[^>]*\bopen(?:[ =>])/);
      expect(markup).not.toContain("internal_fixture_reason");
      expect(markup).not.toContain("quota_cache");
      expect(markup).not.toContain("restore_points");
      expect(markup).not.toContain("provider_mode");
      expect(markup).not.toContain("<button");
    }
  });
});
