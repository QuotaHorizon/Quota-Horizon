import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { QuotaGauge } from "./QuotaGauge";

describe("quota-only gauges", () => {
  it("shows a neutral ring and dash for an absent short window", () => {
    const markup = renderToStaticMarkup(<QuotaGauge remaining={null} label="5h" />);
    expect(markup).toContain('aria-label="5h —"');
    expect(markup).not.toContain("0%");
    expect(markup).not.toContain("100%");
  });
  it("distinguishes real zero from an unavailable quota", () => {
    expect(renderToStaticMarkup(<QuotaGauge remaining={0} label="5h" />)).toContain("0%");
    expect(renderToStaticMarkup(<QuotaGauge remaining={28.3} label="week" />)).toContain("28.3%");
  });
  it("does not add precision to whole-number observed quota", () => {
    const markup = renderToStaticMarkup(<QuotaGauge remaining={98} label="week" />);
    expect(markup).toContain("98%");
    expect(markup).not.toContain("98.0%");
  });
});
