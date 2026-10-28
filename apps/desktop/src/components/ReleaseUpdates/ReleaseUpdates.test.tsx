import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { ReleaseUpdateDialog, shouldNotify, type ReleaseUpdate } from ".";

const release: ReleaseUpdate = {currentVersion:"0.2.0", latestVersion:"0.3.0", available:true,
  notes:"Faster reset updates\n<script>external text</script>",url:"https://github.com/QuotaHorizon/Quota-Horizon/releases/tag/v0.3.0"};
describe("release notices", () => {
  it("respects version-specific dismissal and timed snooze", () => {
    expect(shouldNotify(release, {}, 1000)).toBe(true);
    expect(shouldNotify(release, {ignored:"0.3.0"}, 1000)).toBe(false);
    expect(shouldNotify({...release,latestVersion:"0.4.0"}, {ignored:"0.3.0"}, 1000)).toBe(true);
    expect(shouldNotify(release, {laterUntil:2000}, 1000)).toBe(false);
    expect(shouldNotify(release, {laterUntil:2000}, 2000)).toBe(true);
    expect(shouldNotify({...release,available:false}, {}, 1000)).toBe(false);
  });
  it.each(["zh","en"] as const)("renders release notes as inert text in %s", language => {
    const html=renderToStaticMarkup(<ReleaseUpdateDialog update={release} language={language} onLater={()=>{}} onIgnore={()=>{}} onDownload={()=>{}} />);
    expect(html).not.toContain("<script>");
    expect(html).toContain("&lt;script&gt;");
    expect(html).toContain(language === "zh" ? "查看并下载" : "View and download");
    expect(html).toContain("v0.3.0");
  });
});
