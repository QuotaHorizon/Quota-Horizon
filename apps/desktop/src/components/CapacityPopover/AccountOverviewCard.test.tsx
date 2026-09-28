import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { AccountOverviewCard } from "./AccountOverviewCard";
import type { AccountOverview } from "./accountOverview";
import { accountWeeklyResetDate } from "./presentation";

const now = Date.parse("2026-09-06T10:00:00Z");
function account(remaining: number, target: number): AccountOverview {
  return {
    id: `account-${remaining}`, email: `demo${remaining}@example.com`, note: "", plan: "Pro", active: remaining === 28,
    usage: { primary: { usedPercent: 100 - remaining, remainingPercent: remaining, windowMinutes: 10080, resetsAt: now / 1000 + 2 * 86400 }, fetchedAt: new Date(now).toISOString() },
    workPlan: {
      schemaVersion: "1.0", status: "available", reasonCode: "work_plan_available",
      schedule: { revision: 1, enabled: true, offPeriods: [], updatedAt: new Date(now).toISOString() },
      baseline: { kind: "schedule_baseline", limitId: "managed:weekly", source: "managed_account_cache", observedAt: new Date(now).toISOString(), freshness: "stale", comparison: {
        enabled: true, segment: "working", targetAt: new Date(now + 3600000).toISOString(), expectedRemainingPercent: target,
        actualRemainingPercent: remaining, overspendPercent: 0, state: "on_pace", dailyBudgetPercent: 100 / 7, usableMinutesPerDay: 1440, offMinutesPerDay: 0, windowExpired: false,
      } },
    },
  };
}
function render(row: AccountOverview) {
  return renderToStaticMarkup(<AccountOverviewCard account={row} privacyMode={false} now={now} language="zh" onSwitch={() => { throw new Error("Rendering must not switch accounts"); }} />);
}

describe("saved-account quota and today plan cards", () => {
  it("keeps fractional observations and explicitly calculated target precision", () => {
    const markup = render(account(28.4, 26));
    expect(markup).toContain("28.4%");
    expect(markup).toContain("26.0%");
    expect(render(account(98, 96))).toContain("98%");
    expect(render(account(98, 96))).not.toContain("98.0%");
  });
  it("renders two independent plans together without making the card a switch button", () => {
    const first = render(account(28, 26.4));
    const second = render(account(81, 75.6));
    expect(first).toContain("28%");
    expect(first).toContain("26.4%");
    expect(second).toContain("81%");
    expect(second).toContain("75.6%");
    expect(first).toContain("使用中");
    expect(second).not.toContain("使用中");
    expect(second).toContain("切换使用");
    expect(first).toMatch(/^<article/);
    expect(second).toMatch(/^<article/);
    expect(first).not.toContain("5h");
    expect(first).not.toContain("每日预算");
    expect(first).toContain("周重置");
    expect(first).toContain(accountWeeklyResetDate(now / 1000 + 2 * 86400, "zh"));
  });

  it("keeps quota on a temporary failure and does not falsely demand a login", () => {
    const row = account(28, 26.4);
    row.usage.error = "HTTP 503 Service Unavailable";
    const markup = render(row);
    expect(markup).toContain("28%");
    expect(markup).toContain("保留上次额度");
    expect(markup).not.toContain("登录需处理");
  });

  it("does not make up a plan for an account with no valid weekly snapshot", () => {
    const row = account(81, 75.6);
    row.usage = {};
    row.workPlan!.baseline = null;
    const markup = render(row);
    expect(markup).toContain("等待有效周额度");
    expect(markup).not.toContain("75.6%");
    expect(markup).not.toContain("100%");
  });
});
