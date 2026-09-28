import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { ShortQuotaCard } from "./ShortQuotaCard";

describe("persistent short-window card", () => {
  it("keeps the ring and a dash for Pro, without a false zero or full quota", () => {
    const markup = renderToStaticMarkup(<ShortQuotaCard window={null} notApplicable language="zh" now={0} />);
    expect(markup).toContain('role="img"');
    expect(markup).toContain('aria-label="5h —"');
    expect(markup).toContain("此套餐无独立 5h 限额");
    expect(markup).not.toContain(">0%");
    expect(markup).not.toContain("100%");
  });

  it("renders a real Plus short window to one decimal place", () => {
    const markup = renderToStaticMarkup(<ShortQuotaCard notApplicable={false} language="en" now={0}
      window={{ limitId: "codex:primary", label: "5h", windowMinutes: 300, usedPercent: 12.4, remainingPercent: 87.6, resetsAt: null }} />);
    expect(markup).toContain("87.6%");
    expect(markup).not.toContain("No separate");
  });
});
