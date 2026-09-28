import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { translate, type Language, type Translate } from "../../i18n";
import type { Account, UsageWindow } from "../../types";
import { formatSystemTime } from "../../utils/format";
import { AccountUsageMeter, UsageRefreshAge } from "./UsageMeter";

const now = Date.parse("2026-09-26T04:00:00Z");
const fetchedAt = "2026-09-26T03:55:00Z";
const weekly: UsageWindow = {
  usedPercent: 50.4, remainingPercent: 49.6, windowMinutes: 10_080,
  resetsAt: Date.parse("2026-09-27T08:16:00Z") / 1000,
};
const short: UsageWindow = { usedPercent: 22, remainingPercent: 78, windowMinutes: 300 };
const tFor = (language: Language): Translate => (key, values) => translate(language, key, values);
const account: Pick<Account, "plan" | "usage"> = {
  plan: "prolite", usage: { primary: weekly, fetchedAt },
};

describe("account quota presentation", () => {
  beforeEach(() => { vi.useFakeTimers(); vi.setSystemTime(now); });
  afterEach(() => { vi.useRealTimers(); });

  it("always carries the weekly-only account's last successful read time", () => {
    const markup = renderToStaticMarkup(
      <AccountUsageMeter account={account} slot="weekly" language="zh" t={tFor("zh")} />,
    );
    expect(markup).toContain("49.6%");
    expect(markup).toContain("5 分钟前更新");
    expect(markup).toContain(formatSystemTime(fetchedAt, "zh"));
    expect(markup).toContain("7 天");
  });

  it("keeps the Pro short-window placeholder labelled without inventing a percent", () => {
    const markup = renderToStaticMarkup(
      <AccountUsageMeter account={account} slot="short" variant="card" language="zh" t={tFor("zh")} />,
    );
    expect(markup).toContain("短周期额度");
    expect(markup).toContain("—");
    expect(markup).toContain("此套餐无独立短周期限额");
    expect(markup).not.toContain('role="progressbar"');
    expect(markup).not.toContain("100%");
    expect(markup).not.toContain("0%");
  });

  it("does not mistake missing Plus data for an inapplicable window", () => {
    const markup = renderToStaticMarkup(
      <AccountUsageMeter account={{ plan: "plus", usage: { fetchedAt } }} slot="short" language="zh" t={tFor("zh")} />,
    );
    expect(markup).toContain("尚未读取到此窗口额度");
    expect(markup).not.toContain("无独立短周期");
    expect(markup).not.toContain("刚刚更新");
    expect(markup).not.toContain('role="progressbar"');
  });

  it("uses actual duration instead of primary/secondary position in both views", () => {
    const reversed = { plan: "plus", usage: { primary: weekly, secondary: short, fetchedAt } };
    const shortMarkup = renderToStaticMarkup(
      <AccountUsageMeter account={reversed} slot="short" variant="card" language="zh" t={tFor("zh")} />,
    );
    const weeklyMarkup = renderToStaticMarkup(
      <AccountUsageMeter account={reversed} slot="weekly" language="en" t={tFor("en")} />,
    );
    expect(shortMarkup).toContain("78%");
    expect(shortMarkup).toContain("5 小时");
    expect(shortMarkup).toContain("短周期额度");
    expect(weeklyMarkup).toContain("49.6%");
    expect(weeklyMarkup).toContain("7 d");
    expect(weeklyMarkup).toContain("Updated 5 min ago");
  });

  it("shows a real nonstandard duration and never guesses one from a legacy slot", () => {
    const render = (windowMinutes?: number) => renderToStaticMarkup(
      <AccountUsageMeter account={{ plan: "pro", usage: { primary: { ...weekly, windowMinutes } } }}
        slot="weekly" variant="card" language="en" t={tFor("en")} />,
    );
    expect(render(20_160)).toContain("14 d");
    expect(render()).toContain("Long-window quota");
    expect(render()).not.toContain("7 d");
  });

  it("retains the last value and age after a failure, without exposing a raw error", () => {
    const markup = renderToStaticMarkup(
      <AccountUsageMeter account={{ ...account, usage: { ...account.usage, error: "network_internal_failure" } }}
        slot="weekly" language="zh" t={tFor("zh")} />,
    );
    expect(markup).toContain("49.6%");
    expect(markup).toContain("5 分钟前更新");
    expect(markup).toContain("刷新未成功，保留上次额度");
    expect(markup).not.toContain("network_internal_failure");
    expect(markup).not.toContain("刚刚更新");
  });

  it.each([NaN, Infinity, -1, 101])("does not draw an invalid quota %s as a healthy or empty meter", (remainingPercent) => {
    const markup = renderToStaticMarkup(
      <AccountUsageMeter account={{ ...account, usage: { primary: { ...weekly, remainingPercent } } }}
        slot="weekly" language="en" t={tFor("en")} />,
    );
    expect(markup).toContain("—");
    expect(markup).toContain("No quota reading for this window");
    expect(markup).not.toContain('role="progressbar"');
    expect(markup).not.toContain('class="good"');
  });

  it("retains genuine zero and does not pad observed integers with fake decimals", () => {
    const markup = renderToStaticMarkup(
      <AccountUsageMeter account={{ ...account, usage: { primary: { ...weekly, remainingPercent: 0 } } }}
        slot="weekly" language="zh" t={tFor("zh")} />,
    );
    expect(markup).toContain('role="progressbar"');
    expect(markup).toContain(">0%</strong>");
    expect(markup).not.toContain("0.0%");
  });

  it("keeps an unknown observation time visible instead of silently hiding it", () => {
    for (const timestamp of [undefined, "invalid"]) {
      const markup = renderToStaticMarkup(<UsageRefreshAge fetchedAt={timestamp} language="zh" t={tFor("zh")} />);
      expect(markup).toContain("更新时间未知");
    }
  });

  it("does not call a future-dated observation freshly updated", () => {
    const markup = renderToStaticMarkup(
      <UsageRefreshAge fetchedAt="2026-09-26T05:00:00Z" language="zh" t={tFor("zh")} />,
    );
    expect(markup).toContain("更新时间晚于本机时间");
    expect(markup).not.toContain("刚刚更新");
    expect(markup).not.toContain("0 秒前");
  });

  it.each([
    [15_000, "刚刚更新"], [90_000, "1 分钟前更新"],
    [7_200_000, "2 小时前更新"], [172_800_000, "2 天前更新"],
  ])("formats an age of %s without ever-growing second counts", (elapsed, expected) => {
    const markup = renderToStaticMarkup(
      <UsageRefreshAge fetchedAt={new Date(now - elapsed).toISOString()} language="zh" t={tFor("zh")} />,
    );
    expect(markup).toContain(expected);
  });
});
