import { describe, expect, it } from "vitest";
import type { LegacyMigrationDryRunReport } from "../../types";
import {
  formatMigrationBytes,
  legacyImportedLanguageLabel,
  legacyOptionalStateLabel,
  migrationAccountDispositionColor,
  migrationArtifactStateColor,
  migrationDispositionColor,
  migrationPlanCounts,
  migrationStatusColor,
  selectedMigrationFolderLabel,
} from "./legacyMigration";

describe("legacy migration presentation", () => {
  it("formats bounded byte totals", () => {
    expect(formatMigrationBytes(0)).toBe("0 B");
    expect(formatMigrationBytes(1536)).toBe("1.50 KiB");
    expect(formatMigrationBytes(5 * 1024 * 1024)).toBe("5.00 MiB");
  });

  it("maps status and disposition severity", () => {
    expect(migrationStatusColor("ready")).toBe("green");
    expect(migrationStatusColor("failed")).toBe("red");
    expect(migrationDispositionColor("review")).toBe("gold");
    expect(migrationDispositionColor("skip")).toBe("default");
    expect(migrationArtifactStateColor("present")).toBe("green");
    expect(migrationArtifactStateColor("unsafe")).toBe("red");
    expect(migrationAccountDispositionColor("import")).toBe("green");
    expect(migrationAccountDispositionColor("keep_existing")).toBe("gold");
    expect(migrationAccountDispositionColor("unsupported")).toBe("red");
    expect(migrationAccountDispositionColor("already_present")).toBe("default");
  });

  it("counts every migration disposition", () => {
    const report = {
      plan: [
        { disposition: "import" },
        { disposition: "import" },
        { disposition: "review" },
        { disposition: "blocked" },
      ],
    } as LegacyMigrationDryRunReport;
    expect(migrationPlanCounts(report)).toEqual({ import: 2, review: 1, skip: 0, blocked: 1 });
  });

  it("shows only the selected folder name", () => {
    expect(selectedMigrationFolderLabel("/private/backup/CodexQuotaViewer")).toBe("CodexQuotaViewer");
    expect(selectedMigrationFolderLabel("C:\\backup\\Viewer")).toBe("Viewer");
  });

  it("does not present absent import preferences as disabled", () => {
    expect(legacyOptionalStateLabel(null, "On", "Off")).toBe("—");
    expect(legacyOptionalStateLabel(false, "On", "Off")).toBe("Off");
    expect(legacyOptionalStateLabel(true, "On", "Off")).toBe("On");
    expect(legacyImportedLanguageLabel(null)).toBe("—");
    expect(legacyImportedLanguageLabel("zh")).toBe("中文");
    expect(legacyImportedLanguageLabel("en")).toBe("English");
  });
});
