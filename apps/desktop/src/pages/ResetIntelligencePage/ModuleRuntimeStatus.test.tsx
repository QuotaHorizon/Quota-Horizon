import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { quantitativeFixture } from "../../preview/quantitativeFixture";
import { ModuleRuntimeStatus } from "./ModuleRuntimeStatus";

describe("background progress", () => {
  it("shows pending historical evaluation without claiming it is up to date", () => {
    const view = quantitativeFixture(Date.now());
    view.runtime = {managed:true, state:"running", checkedAt:null, retryAt:null, issue:null, health:"catching_up"};
    const english = renderToStaticMarkup(<ModuleRuntimeStatus view={view} language="en" />);
    expect(english).toContain("Catching up historical evaluation");
    expect(english).not.toContain("up to date");
    expect(english).not.toContain("data-issue");
    expect(renderToStaticMarkup(<ModuleRuntimeStatus view={view} language="zh" />)).toContain("正在补齐历史评估");
  });
  it("does not call a present but stalled process healthy", () => {
    const view = quantitativeFixture(Date.parse("2026-10-30T01:00:00Z"));
    view.runtime = {managed:true, state:"running", checkedAt:null, retryAt:null,
      issue:"evaluation_stalled", health:"stalled", lastCaptureAt:"2026-10-30T01:00:00Z", lastReportAt:"2026-10-30T00:00:00Z"};
    const html = renderToStaticMarkup(<ModuleRuntimeStatus view={view} language="en" />);
    expect(html).toContain("Updates stalled");
    expect(html).toContain("Collected");
    expect(html).toContain("Evaluated");
    expect(html).not.toContain("evaluation_stalled");
    expect(html).not.toContain("up to date");
  });
  it("keeps storage detail collapsed and distinguishes completed maintenance from disk pressure", () => {
    const view = quantitativeFixture(Date.now());
    view.runtime = {managed:true, state:"running", checkedAt:null, retryAt:null, issue:null, health:"healthy",
      storage:{observerBytes:2*1024**3,producerBytes:1024**3,logBytes:1024**2,logLimitBytes:5*1024**2,
        maintenance:[{module:"observer",phase:"done"},{module:"producer",phase:"vacuum",state:"waiting_for_disk_space"}]}};
    const html = renderToStaticMarkup(<ModuleRuntimeStatus view={view} language="zh" />);
    expect(html).toContain("<details>");
    expect(html).not.toContain("<details open");
    expect(html).toContain("3.00 GiB");
    expect(html).toContain("1.0 MiB");
    expect(html).toContain("5.0 MiB");
    expect(html).toContain("保留有效历史");
    expect(html).toContain("等待磁盘空间");
  });
});
