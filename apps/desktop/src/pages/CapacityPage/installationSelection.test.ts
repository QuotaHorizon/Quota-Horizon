import { describe, expect, it } from "vitest";
import type { CodexCandidate, DesktopStatusEnvelope } from "../../../../capacity-preview/src/status";
import { currentInstallationChoice, installationIssueCode, installationLabel, selectableInstallation } from "./installationSelection";
import { capacityIssueMessage } from "./statusCopy";

const candidate = (id: string, verification: CodexCandidate["verification"] = "verified"): CodexCandidate => ({
  executableId: id, canonicalPath: "/Applications/ChatGPT.app/Contents/Resources/codex",
  version: "codex-cli 0.153.4", sources: ["macos_chatgpt_bundle"], verification, requiresConfirmation: true,
});
const envelope = (candidates: CodexCandidate[], selectedExecutableId: string | null = null): DesktopStatusEnvelope => ({
  schemaVersion: "1.0", sequence: 2, lifecycle: "selection_required", status: null,
  candidates, selectedExecutableId, persistenceEnabled: true, issue: null,
});

describe("Codex installation selection after an update", () => {
  it("recognizes current bundled paths after explicit selection loses discovery tags", () => {
    for (const canonicalPath of [
      "/Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex",
      "/Users/example/Apps/Relocated Codex.app/Contents/Resources/codex-cli/bin/codex",
    ]) {
      const app = { ...candidate("app"), canonicalPath, sources: ["explicit"] };
      const cli = { ...candidate("cli"), canonicalPath: "/opt/homebrew/bin/codex", sources: ["path"] };
      expect(installationLabel(app, "zh")).toContain("App · 内置读取组件");
      expect(currentInstallationChoice(envelope([cli, app]))).toBe("app");
    }
    expect(installationLabel({ ...candidate("other"), canonicalPath: "/tmp/example.app/tools/codex", sources: [] }, "en")).toContain("terminal installation");
  });
  it("distinguishes the bundled App reader from a standalone CLI even when both report codex-cli", () => {
    const app = candidate("app");
    const cli = { ...candidate("cli"), canonicalPath: "/opt/homebrew/bin/codex", sources: ["path"] };
    expect(installationLabel(app, "zh")).toContain("桌面 App");
    expect(installationLabel(app, "zh")).toContain("推荐");
    expect(installationLabel(app, "zh")).not.toContain("codex-cli");
    expect(installationLabel(cli, "zh")).toContain("CLI · 终端安装");
    expect(currentInstallationChoice(envelope([cli, app]))).toBe("app");
    expect(currentInstallationChoice(envelope([cli, app]), "cli")).toBe("cli");
  });
  it("defaults to a verified candidate without relying on a separate initial-load state", () => {
    expect(currentInstallationChoice(envelope([candidate("path", "confirmation_required"), candidate("bundle")]))).toBe("bundle");
  });
  it("uses the new identity after a background update even if version and path are unchanged", () => {
    const previous = candidate("old-file");
    const updated = candidate("new-file");
    expect(previous.canonicalPath).toBe(updated.canonicalPath);
    expect(previous.version).toBe(updated.version);
    expect(currentInstallationChoice(envelope([updated]), previous.executableId)).toBe("new-file");
  });
  it("preserves a still-valid manual choice across refreshes and reordered options", () => {
    expect(currentInstallationChoice(envelope([candidate("bundle"), candidate("path", "confirmation_required")]), "path")).toBe("path");
    expect(currentInstallationChoice(envelope([candidate("path", "confirmation_required"), candidate("bundle")]), "path")).toBe("path");
  });
  it("prefers the current selected installation when there is no valid manual choice", () => {
    expect(currentInstallationChoice(envelope([candidate("a"), candidate("b")], "b"), "removed")).toBe("b");
  });
  it("does not submit a failed candidate just because the old choice is nonempty", () => {
    for (const verification of ["version_failed", "version_timeout"] as const) {
      const failed = candidate("old-file", verification);
      expect(selectableInstallation(failed)).toBe(false);
      expect(currentInstallationChoice(envelope([failed]), "old-file")).toBeUndefined();
      expect(currentInstallationChoice(envelope([failed, candidate("replacement")]), "old-file")).toBe("replacement");
    }
  });
  it("disables confirmation when discovery has no available options", () => {
    expect(currentInstallationChoice(null, "old-file")).toBeUndefined();
    expect(currentInstallationChoice(envelope([]), "old-file")).toBeUndefined();
  });
  it("keeps a specific safe rejection instead of replacing it with a generic selection prompt", () => {
    const code = installationIssueCode({ code: "codex_candidate_changed", message: "untrusted detail" });
    expect(capacityIssueMessage(code, "zh")).toBe("Codex 安装已更新，请重新确认读取来源。");
  });
  it("never renders raw rejection text or internal codes", () => {
    for (const error of ["raw private error", { message: "raw private error" }, { code: "unrecognized_internal_code", message: "raw private error" }, null]) {
      const copy = capacityIssueMessage(installationIssueCode(error), "zh");
      expect(copy).not.toContain("raw private");
      expect(copy).not.toContain("unrecognized_internal_code");
    }
  });
});
