import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { DesktopStatusEnvelope } from "../../../../capacity-preview/src/status";
import { PublicResetArchiveView } from "./ArchiveView";
import { LocalResetFacts } from "./LocalResetPanel";
import { evidenceDispositionLabel, localResetFacts, publicEvidenceTime, publicEvidenceUrl } from "./presentation";
import type { PublicResetArchive, PublicResetArchiveEntry } from "./types";

vi.mock("../../api/backend", () => ({ isDesktopApp: false }));

function entry(kind: PublicResetArchiveEntry["signal"]["kind"], disposition: PublicResetArchiveEntry["disposition"]): PublicResetArchiveEntry {
  return {
    disposition,
    title: { zh: "测试发放说明", en: "Synthetic grant notice" },
    summary: { zh: "这是合成测试资料。", en: "Synthetic evidence only." },
    interpretation: { zh: "不能证明本机账号到账。", en: "Not proof of receipt." },
    signal: {
      signalId: "synthetic_signal", revision: 1, evidenceFamilyId: "synthetic_family", eventId: null,
      kind, scope: "broad_codex", recordedAt: "2026-09-07T04:00:00Z", occurredAt: null,
      source: {
        canonicalUrl: "https://help.openai.com/en/articles/20001498-how-banked-codex-resets-work",
        author: "OpenAI", review: "primary_reviewed", sourceClass: "official_documentation",
        publishedAt: null, collectedAt: "2026-09-07T04:00:00Z", contentSha256: "a".repeat(64),
        parserVersion: "synthetic_parser", discoveredVia: [],
      },
    },
  };
}

function archive(): PublicResetArchive {
  return {
    schemaVersion: 1, mode: "bundled_archive", reviewedAt: "2026-09-07T04:00:00Z",
    entries: [entry("global_banked_reset_grant", "confirmed")],
    evidenceFamilyCount: 1, historyComplete: false, forecastStatus: "abstained",
  };
}

function envelope(): DesktopStatusEnvelope {
  return {
    schemaVersion: "1.0", sequence: 3, lifecycle: "ready", candidates: [], selectedExecutableId: null,
    issue: null, persistenceEnabled: true,
    status: {
      schemaVersion: "1.0", capturedAt: "2026-09-07T04:00:00Z", codexVersion: null,
      account: { authMode: "chatgpt", planType: "pro", bindingStatus: "stable" },
      dataStatus: { availability: "complete", freshness: "live", compatibility: "tested", reasonCodes: [] },
      quotaWindows: [
        { limitId: "codex:weekly", label: "weekly", windowMinutes: 10_080, usedPercent: 71.6, remainingPercent: 28.4, resetsAt: "2026-09-08T05:04:14Z" },
        { limitId: "spark:weekly", label: "Spark", windowMinutes: 10_080, usedPercent: 0, remainingPercent: 100, resetsAt: "2026-09-09T00:00:00Z" },
      ],
      resetCredits: { summaryStatus: "available", availableCount: 2, detailsStatus: "complete" },
      usage: { availability: "complete", hasSummary: false, reasonCodes: [] }, diagnosticCodes: [],
    },
  };
}

describe("reset intelligence presentation", () => {
  it("labels the dated archive as non-live and never invents numeric probabilities", () => {
    const markup = renderToStaticMarkup(<PublicResetArchiveView archive={archive()} language="zh" />);
    expect(markup).toContain("应用附带资料");
    expect(markup).toContain("归档截至");
    expect(markup).not.toContain("未来 24 小时");
    expect(markup).not.toContain("未来 48 小时");
    expect(markup).not.toContain("Horizon 自有预测模型");
    expect(markup).not.toContain("0%");
    expect(markup).not.toContain("93%");
    expect(markup).not.toContain("synthetic_signal");
    expect(markup).not.toContain("synthetic_parser");
  });

  it("does not present the confirmed grant as a confirmed full reset", () => {
    const markup = renderToStaticMarkup(<PublicResetArchiveView archive={archive()} language="zh" />);
    expect(markup).toContain("尚无已核实的完整重置公告");
    expect(markup).toContain("重置卡发放");
    expect(markup).toContain("可保存、主动使用");
  });

  it("shows indirect evidence as a lead without leaking internal enum names", () => {
    const data = archive();
    data.entries[0].disposition = "needs_review";
    data.entries[0].signal.source.review = "indirect";
    const markup = renderToStaticMarkup(<PublicResetArchiveView archive={data} language="zh" />);
    expect(markup).toContain("待核实线索");
    expect(markup).not.toContain("needs_review");
    expect(evidenceDispositionLabel("unexpected_internal_state", "zh")).toBe("待核实线索");
  });

  it("leaves unavailable Pro short-window quota blank and preserves weekly decimals", () => {
    const markup = renderToStaticMarkup(<LocalResetFacts envelope={envelope()} language="zh" now={Date.parse("2026-09-07T04:01:00Z")} />);
    expect(markup).toContain("28.4%");
    expect(markup).toContain("5h");
    expect(markup).toContain("无独立 5h 限额");
    expect(markup).not.toContain("100.0%");
    expect(markup).toContain("正在读取额度变化");
  });

  it("never substitutes Spark when the canonical account window is absent", () => {
    const data = envelope();
    data.status!.quotaWindows = data.status!.quotaWindows.filter((window) => window.limitId.startsWith("spark"));
    expect(localResetFacts(data).weekly).toBeNull();
    expect(localResetFacts(null).weekly).toBeNull();
  });

  it("distinguishes stale or failed quota from public archive freshness", () => {
    const data = envelope();
    data.lifecycle = "stale";
    expect(localResetFacts(data).fresh).toBe(false);
    const markup = renderToStaticMarkup(<LocalResetFacts envelope={data} language="zh" now={0} failed />);
    expect(markup).toContain("显示上次读数");
    expect(markup).toContain("28.4%");
    expect(markup).not.toContain("已核实公告");
  });

  it("guards links and unknown dates without executing remote content", () => {
    for (const url of ["javascript:alert(1)", "http://openai.com/", "https://localhost/", "https://openai.com.evil.test/", "https://user:secret@x.com/"]) {
      expect(publicEvidenceUrl(url)).toBeNull();
    }
    expect(publicEvidenceUrl("https://quotaresets.com/api/v1/events.json")).not.toBeNull();
    expect(publicEvidenceTime("bad-date", "zh")).toBe("—");
  });
});
