import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { ResetBriefing } from "./ResetBriefing";
import { forecastPresentation, postReading, resetRelated } from "./resetBriefingModel";
import { ResetNoticeChip } from "../../components/CapacityPopover/ResetNoticeChip";
import type { PublicInsights, PublicPost, PublicResetTimeline } from "./types";

const now = Date.parse("2026-09-23T03:00:00Z");
const post: PublicPost = { id: "123", url: "https://x.com/thsottiaux/status/123",
  text: "OK fine. But it’s also still coming in Tuesday.", translatedText: "好吧。但它周二也会到。",
  publishedAt: "2026-09-22T04:00:00Z", kind: "context", isReply: true,
  parent: { id: "122", author: "someone", url: "https://x.com/someone/status/122",
    text: "You owe us a banked reset. Please.", checkedAt: "2026-09-23T02:59:00Z" } };
function insights(): PublicInsights {
  return { forecast: { attemptedAt: "2026-09-23T02:59:00Z", issue: null, value: {
    sourceUrl: "https://codex-reset.com/api/forecast", updatedAt: "2026-09-23T02:58:00Z", checkedAt: "2026-09-23T02:59:00Z",
    probability24h: 20, probability48h: 35, confidence: "low", mode: "model", lastResetAt: null,
    signalScore: 93, signalUrl: post.url, signalPublishedAt: post.publishedAt,
    signalDeadline: "2026-09-23T06:59:59Z", signalCorrected: false,
  } }, posts: { fetchedAt: "2026-09-23T02:59:00Z", checkedAt: "2026-09-23T02:59:00Z", posts: [post] } };
}
function timeline(): PublicResetTimeline {
  return { sources: [], entries: [], revisionCount: 0, evidenceFamilyCount: 0, insights: insights() };
}
const render = (value = timeline()) => renderToStaticMarkup(<ResetBriefing timeline={value} language="zh" now={now} />);

describe("attributed reset briefing", () => {
  it("shows external 24/48h estimates separately from a rule-based signal score", () => {
    const html = render();
    for (const copy of ["未来 24 小时", "未来 48 小时", "20%", "35%", "信号评分 93/100", "网站自评置信度：低", "网站更新", "本机读取", "Codex Reset"]) expect(html).toContain(copy);
    expect(html).not.toContain("93%");
    expect(html).toContain("发言强度评分，满分 100");
    expect(html.indexOf("20%")).toBeLessThan(html.indexOf("Tibo · 相关发言"));
    expect(html.match(/好吧。但它周二也会到。/gu)).toHaveLength(1);
    expect(html).toContain("<summary>预测说明</summary>");
  });
  it("renders a reply with its actual parent, source translation and full original", () => {
    const html = render();
    expect(html).toContain("You owe us a banked reset. Please.");
    expect(html).toContain("好吧。但它周二也会到。");
    expect(html).toContain("OK fine. But it’s also still coming in Tuesday.");
    expect(html).toContain("父帖来源"); expect(html).toContain("译文来自 Codex Reset");
    expect(resetRelated(post)).toBe(true);
    expect(postReading(post, "zh")).toMatchObject({tone: "grant"});
    expect(html).toContain("回复上文讨论的重置卡");
  });
  it("does not make up missing context or expose arbitrary HTML", () => {
    const data = timeline();
    data.insights!.posts!.posts = [{ ...post, parent: { ...post.parent!, text: null }, translatedText: null, text: "<script>reset()</script>" }];
    const html = render(data);
    expect(html).toContain("父帖暂未取得"); expect(html).toContain("回复上文暂缺");
    expect(html).toContain("&lt;script&gt;"); expect(html).not.toContain("<script>");
    expect(html).not.toContain("应按重置卡");
  });
  it("retains old values on failure without passing them off as current or replacing missing data with zero", () => {
    const data = timeline(); data.insights!.forecast.issue = "request_failed";
    const html = render(data);
    expect(html).toContain("20%"); expect(html).toContain("上次结果 · 待更新");
    expect(html).not.toContain("request_failed");
    expect(forecastPresentation(data.insights, now).signalActive).toBe(false);
    data.insights!.forecast.value = null;
    expect(render(data)).toContain("预测来源暂不可用");
    expect(render(data)).not.toContain("<strong>0%</strong>");
  });
  it("ages by source generation and fetch times, invalidates expired and corrected scores", () => {
    const data = insights();
    expect(forecastPresentation(data, now)).toMatchObject({stale: false, signalActive: true});
    expect(forecastPresentation(data, now + 31 * 60_000).stale).toBe(true);
    data.forecast.value!.signalDeadline = "2026-09-23T02:00:00Z";
    expect(forecastPresentation(data, now).signalActive).toBe(false);
    data.forecast.value!.signalDeadline = "2026-09-23T04:00:00Z";
    data.forecast.value!.signalCorrected = true;
    expect(forecastPresentation(data, now).signalActive).toBe(false);
    data.forecast.value!.checkedAt = "2026-09-23T01:00:00Z";
    expect(forecastPresentation(data, now).stale).toBe(true);
  });
  it("uses the same forecast in the menu, attributed and without changing quota", () => {
    const html = renderToStaticMarkup(<ResetNoticeChip insights={insights()} now={now} language="zh" onOpen={() => undefined} />);
    expect(html).toContain("第三方 24h 20%"); expect(html).toContain("Codex Reset");
    expect(html).not.toContain("93%"); expect(html).not.toContain("100%");
  });
  it("describes a grant as manual-use cards, not an automatic account refill", () => {
    const grant = { ...post, isReply: false, parent: null, kind: "grant" as const, text: "We are also loading a banked reset for Plus users." };
    expect(postReading(grant, "zh")).toMatchObject({title: "Tibo 宣布开始发放重置卡", tone: "grant"});
    expect(postReading(grant, "zh").meaning).toContain("重置卡可手动使用");
  });
  it("does not let casual parent-linked replies displace a substantive announcement", () => {
    const casual = { ...post, text: "pizza plutot", translatedText: "pizza plutot", publishedAt: "2026-09-23T02:34:00Z" };
    expect(resetRelated(casual)).toBe(false);
    expect(resetRelated(post)).toBe(true); // A timing reply remains useful with its banked-reset parent.
    const data = timeline();
    data.insights!.posts!.posts = [casual, { ...post, id: "124", isReply: false, parent: null, kind: "grant", text: "We are also loading a banked reset.", translatedText: null }];
    const html = render(data);
    expect(html).toContain("Tibo 宣布开始发放重置卡");
    expect(html).not.toContain("pizza plutot");
  });
});
