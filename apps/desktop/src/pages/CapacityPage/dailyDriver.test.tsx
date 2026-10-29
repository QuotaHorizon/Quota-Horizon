import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { DesktopStatusEnvelope } from "../../../../capacity-preview/src/status";
import { PlanningExperiments } from "./PlanningExperiments";
import { resetCreditPresentation } from "./resetCreditPresentation";
import { CapacityDetails } from "./CapacityDetails";
import { LocalResetFacts } from "../ResetIntelligencePage/LocalResetPanel";
import { capacityQuotaNotice } from "./statusCopy";
import { CapacityPage } from "./index";
import { getCapacityStatusSnapshot } from "../../api/backend";

vi.mock("../../api/backend", () => ({ isDesktopApp: true, hasLocalBackend: true, getCapacityStatusSnapshot: vi.fn() }));

function envelope(): DesktopStatusEnvelope {
  return { schemaVersion: "1.0", sequence: 1, lifecycle: "ready", candidates: [], selectedExecutableId: "test-reader",
    selectedExecutableSource: "cli", issue: null, persistenceEnabled: true,
    status: { schemaVersion: "1.0", capturedAt: "2026-09-26T14:00:00Z", codexVersion: "codex-cli test",
      account: { authMode: "chatgpt", planType: "pro", bindingStatus: "stable" },
      dataStatus: { availability: "partial", freshness: "live", compatibility: "not_tested", reasonCodes: ["reset_credit_summary_unavailable"] },
      quotaWindows: [], resetCredits: { summaryStatus: "unavailable", availableCount: null, detailsStatus: "unavailable" },
      usage: { availability: "unsupported", hasSummary: false, reasonCodes: [] }, diagnosticCodes: [] } };
}

describe("daily-driver priority and reset-card clarity", () => {
  it.each(["zh", "en"] as const)("shows the live Pro Lite plan with a weekly-only quota in %s", (language) => {
    const value = envelope();
    value.status!.account!.planType = "prolite";
    value.status!.quotaWindows = [{ limitId: "codex:primary", label: "weekly", windowMinutes: 10080,
      usedPercent: 28, remainingPercent: 72, resetsAt: "2026-10-06T12:00:00Z" }];
    vi.mocked(getCapacityStatusSnapshot).mockReturnValue(value);
    const html = renderToStaticMarkup(<CapacityPage language={language} notify={() => undefined} />);
    expect(html).toContain("Pro Lite");
    expect(html).not.toContain("Free");
    expect(html).toContain(language === "zh" ? "此套餐无独立 5h 限额" : "No separate 5h limit on this plan");
  });
  it("does not turn a real weekly-only Pro Lite limit into a five-hour quota", () => {
    const value = envelope();
    value.status!.account!.planType = "prolite";
    value.status!.quotaWindows = [{ limitId: "codex:primary", label: "weekly", windowMinutes: 10080,
      usedPercent: 23, remainingPercent: 77, resetsAt: "2026-10-20T04:59:08Z" }];
    value.status!.resetCredits = { summaryStatus: "available", availableCount: 3, detailsStatus: "complete" };
    vi.mocked(getCapacityStatusSnapshot).mockReturnValue(value);
    const html = renderToStaticMarkup(<CapacityPage language="zh" notify={() => undefined} />);
    expect(html).toContain("77%");
    expect(html).toContain("3 张");
    expect(html).not.toContain('aria-label="5h 77%"');
    expect(html).toContain("此套餐无独立 5h 限额");
  });
  it("keeps quota, reset cards and history on the daily page, without lab entry points", () => {
    vi.mocked(getCapacityStatusSnapshot).mockReturnValue(envelope());
    const html = renderToStaticMarkup(<CapacityPage language="zh" notify={() => undefined} />);
    expect(html.indexOf("额度历史")).toBeGreaterThan(html.indexOf("重置卡"));
    expect(html).not.toContain("规划实验室");
    expect(html).not.toContain("未验收");
    for (const title of ["近期节奏", "我的工作需求", "实际使用记录", "过去的推算与实际"]) expect(html).not.toContain(title);
  });
  it.each(["zh", "en"] as const)("keeps experiments closed and unmounted in %s", (language) => {
    const ExperimentalReader = vi.fn(() => <p>近期节奏 / 实际使用记录</p>);
    const html = renderToStaticMarkup(<PlanningExperiments language={language}><ExperimentalReader /></PlanningExperiments>);
    expect(html).toContain(language === "zh" ? "规划实验室" : "Planning lab");
    expect(html).not.toMatch(/<details[^>]*\bopen(?:[ =>])/);
    expect(html).not.toContain("近期节奏");
    expect(ExperimentalReader).not.toHaveBeenCalled();
  });
  it.each(["zh", "en"] as const)("shows card results without diagnostic prose in %s", (language) => {
    const value = envelope();
    value.status!.resetCredits = { summaryStatus: "available", availableCount: 2, detailsStatus: "complete" };
    vi.mocked(getCapacityStatusSnapshot).mockReturnValue(value);
    const html = renderToStaticMarkup(<CapacityPage language={language} notify={() => undefined} />);
    expect(html).toContain(language === "zh" ? "重置卡" : "Reset credits");
    expect(html).toContain(language === "zh" ? "2 张" : ">2<");
    expect(html).toContain(language === "zh" ? "可在 Codex 中手动使用" : "Redeem manually in Codex");
    for (const text of ["读取状态", "详情状态", "已验证", "Read status", "Planning lab", "规划实验室"]) expect(html).not.toContain(text);
  });
  it("explains missing CLI data without pretending zero or asking to refresh repeatedly", () => {
    const view = resetCreditPresentation(envelope(), "zh");
    expect(view.count).toBeNull();
    expect(view.message).toContain("CLI 未提供");
    expect(view.message).toContain("App 内置读取组件");
    expect(view.message).not.toContain("重试");
    expect(capacityQuotaNotice(envelope(), false, "zh")?.message).toContain("未提供重置卡数据");
  });
  it("preserves a genuine zero and marks retained counts as old on read failure", () => {
    const value = envelope();
    value.status!.resetCredits = { summaryStatus: "available", availableCount: 0, detailsStatus: "complete" };
    expect(resetCreditPresentation(value, "zh").count).toBe(0);
    value.status!.resetCredits.availableCount = 3;
    value.lifecycle = "stale";
    expect(resetCreditPresentation(value, "zh")).toMatchObject({ count: 3, label: "上次读数" });
    expect(resetCreditPresentation(value, "en").message).toContain("last saved");
  });
  it("does not diagnose reader incompatibility while the source is not ready", () => {
    const value = envelope(); value.lifecycle = "selection_required";
    expect(resetCreditPresentation(value, "zh").message).toContain("尚未取得");
    expect(resetCreditPresentation(value, "zh").message).not.toContain("CLI 未提供");
    expect(resetCreditPresentation(null, "en").count).toBeNull();
  });
  it("uses the same missing-credit explanation on the reset page and never calls an incomplete read complete", () => {
    const value = envelope();
    const html = renderToStaticMarkup(<LocalResetFacts envelope={value} language="zh" now={Date.now()} />);
    expect(html).toContain(resetCreditPresentation(value, "zh").message);
    value.status!.dataStatus.availability = "complete"; // Older hosts/cached captures.
    const details = renderToStaticMarkup(<CapacityDetails envelope={value} language="zh" diagnostics={null} vault={null} onInspect={() => undefined} />);
    expect(details).toContain("部分可用");
    expect(details).not.toContain(">完整<");
  });

  it.each(["zh", "en"] as const)("renders the nearest credit expiry in the existing capacity card in %s", (language) => {
    const value = envelope();
    value.status!.resetCredits = { summaryStatus: "available", availableCount: 2, detailsStatus: "complete",
      nextExpiresAt: "2099-11-01T00:00:00Z", knownExpiryCount: 2, expiryDetailsComplete: true };
    vi.mocked(getCapacityStatusSnapshot).mockReturnValue(value);
    const html = renderToStaticMarkup(<CapacityPage language={language} notify={() => undefined} />);
    expect(html).toContain(language === "zh" ? "2 张" : ">2<");
    expect(html).toContain(language === "zh" ? "到期" : "Expires");
    expect(html).toContain(language === "zh" ? "最近到期" : "Next expiry");
    expect(html).toContain("2099");
  });

  it("marks stale expiry readings and rechecks elapsed deadlines without changing captured counts", () => {
    const value = envelope();
    value.status!.resetCredits = { summaryStatus: "available", availableCount: 2, detailsStatus: "complete",
      nextExpiresAt: "2026-10-31T00:00:00Z", knownExpiryCount: 1, expiryDetailsComplete: false };
    value.lifecycle = "stale";
    const now = Date.parse("2026-10-30T00:00:00Z");
    const saved = resetCreditPresentation(value, "zh", now);
    expect(saved.label).toBe("上次读数");
    expect(saved.expiry?.text).toContain("记录");
    expect(saved.expiry?.text).toContain("已知");
    expect(saved.expiry?.title).toContain("2 张中 1 张");
    const elapsed = resetCreditPresentation(value, "en", Date.parse("2026-11-01T00:00:00Z"));
    expect(elapsed.expiry?.text).toBe("Saved · Expiry needs refresh");
    expect(elapsed.count).toBe(2);
  });
});
