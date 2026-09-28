import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { SearchCoverage, searchCoverageCopy, searchEmptyCopy } from "./SearchCoverage";

const complete = { totalSessions: 10, searchedSessions: 8, incompleteSessions: 0, skippedRecords: 0, completedSessions: 10, pendingSessions: 0, decodedBytes: 1048576 };

describe("local session search coverage", () => {
  it("explains linked history and timestamp excerpts for complete searches", () => {
    const copy = searchCoverageCopy(complete, 2, "zh");
    expect(copy.title).toContain("2 条会话");
    expect(copy.title).toContain("历史分段");
    expect(copy.description).toContain("原消息时间");
    expect(copy.type).toBe("info");
  });

  it("never presents an incomplete zero-match search as proof of absence", () => {
    const copy = searchCoverageCopy({ ...complete, incompleteSessions: 3, skippedRecords: 2 }, 0, "zh");
    expect(copy.type).toBe("warning");
    expect(copy.title).toContain("3 条未完整搜索");
    expect(copy.description).toContain("部分文件变动、缺失或超过读取上限");
    expect(copy.description).toContain("2 条过大或不完整记录");
    expect(searchEmptyCopy(true, "zh")).toContain("尚未完整搜索");
    expect(searchEmptyCopy(false, "zh")).toBe("没有匹配的会话");
  });

  it("keeps both languages free of internal status identifiers", () => {
    const copy = searchCoverageCopy({ ...complete, incompleteSessions: 1 }, 0, "en");
    expect(copy.description).toContain("Some files changed, are missing or exceed the read limit");
    expect(searchEmptyCopy(true, "en")).toContain("not fully searched");
    expect(JSON.stringify(copy)).not.toMatch(/incomplete_sessions|search_partial|read_limit/);
  });

  it("has no search notice for an unfiltered session list", () => {
    expect(renderToStaticMarkup(<SearchCoverage coverage={null} matches={10} language="zh" />)).toBe("");
  });

  it("shows stable-list search feedback without presenting previous coverage as current", () => {
    const html = renderToStaticMarkup(<SearchCoverage coverage={null} matches={2} language="zh" searching />);
    expect(html).toContain("正在搜索本地会话");
    expect(html).toContain("保留上次结果");
    expect(html).not.toContain("已包含关联的历史分段");
  });

  it("reports actual progress and exposes a stop action while batches run", () => {
    const html = renderToStaticMarkup(<SearchCoverage coverage={{ ...complete, completedSessions: 4, pendingSessions: 6 }} matches={2} language="zh" searching onStop={() => undefined} />);
    expect(html).toContain("4 / 10");
    expect(html).toContain("1.0 MiB");
    expect(html).toContain("停止搜索");
    expect(html).not.toContain("已包含关联的历史分段");
  });

  it("does not label interrupted or failed continuations as complete coverage", () => {
    expect(searchCoverageCopy({ ...complete, pendingSessions: 6 }, 2, "zh").title).toContain("6 条尚未搜索完");
    const html = renderToStaticMarkup(<SearchCoverage coverage={complete} matches={2} language="zh" stopped onRestart={() => undefined} />);
    expect(html).toContain("搜索已停止");
    expect(html).toContain("重新搜索");
    expect(html).not.toContain("已包含关联的历史分段");
  });

  it("keeps completed results visible and offers an explicit refresh when sources change", () => {
    const html = renderToStaticMarkup(<SearchCoverage coverage={complete} matches={2} language="zh" stale onRestart={() => undefined} />);
    expect(html).toContain("当前搜索结果保持不变");
    expect(html).toContain("刷新搜索");
  });
});
