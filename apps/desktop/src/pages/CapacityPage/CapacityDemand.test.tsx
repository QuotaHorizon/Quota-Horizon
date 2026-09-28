import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { DesktopDemandPlanEnvelope, DesktopDemandPlanUpdate } from "../../../../capacity-preview/src/status";
import { acceptDemandPlan, demandFromForm, demandMessage, localPlanInput, planTime } from "./demandPlanModel";
import { createDemandPlanStore } from "./demandPlanStore";
import { CapacityDemandView } from "./CapacityDemandView";

const now = Date.parse("2026-09-26T04:00:00Z");
const demand = { horizonEnd: "2026-09-28T04:00:00.000Z", demandKind: "active_hours" as const, plannedCodexActiveHours: 8 };
function envelope(revision = 1): DesktopDemandPlanEnvelope {
  return { schemaVersion: "1.0", historyContextId: "context-a", status: "available", reasonCode: "plan_available", generatedAt: new Date(now).toISOString(), observedAt: new Date(now).toISOString(), freshness: "live", decision: "not_assessed", hasOtherEnvironmentPlans: false,
    plans: [{ workPlanId: "synthetic-work-plan", revision, demandId: "synthetic-demand", demandRevision: revision, enabled: true, demand, createdAt: new Date(now).toISOString() }],
    allowances: [{ limitId: "codex:weekly", windowMinutes: 10080, remainingPercent: 40, resetsAt: demand.horizonEnd, allocation: { method: "equal_allocation_v1", reasonCode: "allowance_available", dailyPercent: 20, perPlannedHourPercent: 5 } }],
  };
}
const request: DesktopDemandPlanUpdate = { historyContextId: "context-a", expectedRevision: 1, enabled: true, demand };
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>((yes) => { resolve = yes; }); return { promise, resolve }; }
function render(value: DesktopDemandPlanEnvelope | null, overrides: Partial<Parameters<typeof CapacityDemandView>[0]> = {}) {
  return renderToStaticMarkup(<CapacityDemandView contextId="context-a" language="zh" value={value} now={now} onSave={async () => true} onRetry={() => undefined} {...overrides} />);
}

describe("explicit account work demand", () => {
  it("converts local input to UTC without treating planned time as observation", () => {
    const local = localPlanInput(demand.horizonEnd);
    expect(demandFromForm(local, "active_hours", "8", now)).toEqual(demand);
    expect(demandFromForm(local, "maintain_recent_pace", "junk", now)).toEqual({ ...demand, demandKind: "maintain_recent_pace", plannedCodexActiveHours: null });
    for (const hours of ["", "0", "-1", "Infinity", "NaN", "49"]) expect(demandFromForm(local, "active_hours", hours, now)).toBeNull();
  });
  it("rejects invalid dates, past deadlines, and timezone suffixes in local controls", () => {
    for (const date of ["invalid", "2026-02-30T00:00", "2026-09-20T04:00", "2028-09-26T04:00", demand.horizonEnd]) expect(demandFromForm(date, "maintain_recent_pace", "", now)).toBeNull();
    expect(localPlanInput("invalid")).toBe("");
    expect(planTime("invalid", "en")).toBe("—");
  });
  it("uses the system timezone and rejects a DST gap rather than moving the time", () => {
    const before = process.env.TZ;
    try {
      process.env.TZ = "America/New_York";
      expect(demandFromForm("2026-03-08T02:30", "maintain_recent_pace", "", Date.parse("2026-03-07T00:00:00Z"))).toBeNull();
      expect(demandFromForm("2026-03-08T03:30", "maintain_recent_pace", "", Date.parse("2026-03-07T00:00:00Z"))?.horizonEnd).toBe("2026-03-08T07:30:00.000Z");
      expect(planTime("2026-03-08T07:30:00Z", "en")).toContain("03:30");
    } finally { if (before === undefined) delete process.env.TZ; else process.env.TZ = before; }
  });
  it("keeps revisions monotonic and clears data on context/authentication loss", () => {
    expect(acceptDemandPlan(envelope(3), envelope(1), "context-a")?.plans[0].revision).toBe(3);
    expect(acceptDemandPlan(envelope(), { ...envelope(), historyContextId: "context-b" }, "context-a")).toBeNull();
    const denied = { ...envelope(), status: "unavailable" as const, reasonCode: "history_keychain_denied", plans: [], allowances: [] };
    expect(acceptDemandPlan(envelope(), denied, "context-a")).toBe(denied);
  });
  it("distinguishes an allowance from a forecast and uses no internal identifiers", () => {
    const html = render(envelope());
    expect(html).toContain("计划主动使用 8 小时"); expect(html).toContain("20%"); expect(html).toContain(" / 每 24 小时");
    expect(html).toContain("按计划时间均摊当前余额"); expect(html).toContain("均摊预算");
    for (const internal of ["active_hours", "equal_allocation_v1", "not_assessed", "synthetic-demand", "codex:weekly"]) expect(html).not.toContain(internal);
    expect(render(envelope(), { language: "en" })).toContain("Evenly allocated budget");
  });
  it("hides allowances on read failure, pause, expiration or missing quota", () => {
    expect(render(envelope(), { failed: true })).not.toContain("20%");
    const paused = envelope(); paused.plans[0].enabled = false;
    expect(render(paused)).not.toContain("20%"); expect(render(paused)).toContain("已暂停");
    expect(render(envelope(), { now: Date.parse(demand.horizonEnd) })).toContain("已到期");
    expect(render({ ...envelope(), allowances: [] })).toContain("暂不计算预算");
  });
  it("does not invent post-reset capacity or hide a prior-environment plan", () => {
    const value = envelope(); value.allowances[0].allocation = { method: "equal_allocation_v1", reasonCode: "reset_before_deadline", dailyPercent: null, perPlannedHourPercent: null };
    expect(render(value)).toContain("不将下一周期额度提前算入");
    expect(render({ ...value, plans: [], hasOtherEnvironmentPlans: true })).toContain("有旧版 Codex 的计划");
  });
  it("shows the same compact intent without placing the full editor in the menu", () => {
    const html = render(envelope(), { compact: true, onOpen: () => undefined });
    expect(html).toContain("计划主动使用 8 小时"); expect(html).toContain("设置"); expect(html).not.toContain("均摊预算");
    expect(html).not.toContain("<form");
  });
  it("retains pause/error guidance and newest five-version history without raw IDs", () => {
    const value = envelope(2); value.plans.push(envelope(1).plans[0]); value.status = "revision_conflict"; value.reasonCode = "plan_revision_conflict";
    const html = render(value); expect(html).toContain("另一处已修改计划"); expect(html).toContain("第 1 版");
    expect(demandMessage("some_internal_code", "zh")).not.toContain("some_internal_code");
    expect(demandMessage("history_keychain_denied", "zh")).toContain("旧计划已保留");
  });
  it("coalesces disk reads but honors an event arriving during the old read", async () => {
    const pending = deferred<DesktopDemandPlanEnvelope>(); let changed!: () => void;
    const read = vi.fn().mockImplementationOnce(() => pending.promise).mockResolvedValue(envelope(2));
    const store = createDemandPlanStore("context-a", { read, update: async () => envelope(), subscribe: (callback) => { changed = callback; return () => undefined; } });
    const dispose = store.subscribe(() => undefined); const task = store.reload(); changed(); pending.resolve(envelope()); await task;
    expect(read).toHaveBeenCalledTimes(2); expect(store.getSnapshot().value?.plans[0].revision).toBe(2); dispose();
  });
  it("ignores a pre-save read, rejects cross-context submissions, and preserves conflict", async () => {
    const pending = deferred<DesktopDemandPlanEnvelope>(); let updated = { ...envelope(2), status: "updated" as DesktopDemandPlanEnvelope["status"] };
    const api = { read: vi.fn().mockImplementationOnce(() => pending.promise).mockImplementation(async () => updated), update: vi.fn(async () => updated) };
    const store = createDemandPlanStore("context-a", api); const read = store.reload();
    expect(await store.save({ ...request, historyContextId: "context-b" })).toBe(false); expect(api.update).not.toHaveBeenCalled();
    expect(await store.save(request)).toBe(true); pending.resolve(envelope()); await read;
    expect(store.getSnapshot().value?.plans[0].revision).toBe(2);
    updated = { ...envelope(3), status: "revision_conflict", reasonCode: "plan_revision_conflict" };
    expect(await store.save(request)).toBe(false); expect(store.getSnapshot().value?.status).toBe("revision_conflict");
  });
  it("does not duplicate a pending save, retains failed input's base, and permits retry", async () => {
    const pending = deferred<DesktopDemandPlanEnvelope>();
    const api = { read: vi.fn(async () => envelope()), update: vi.fn().mockImplementationOnce(() => pending.promise).mockRejectedValueOnce(new Error("private failure")) };
    const store = createDemandPlanStore("context-a", api); await store.reload();
    const saving = store.save(request); expect(await store.save(request)).toBe(false);
    pending.resolve({ ...envelope(2), status: "updated" }); expect(await saving).toBe(true);
    expect(await store.save({ ...request, expectedRevision: 2 })).toBe(false);
    expect(store.getSnapshot().value?.plans[0].revision).toBe(2);
    expect(store.getSnapshot().value?.reasonCode).toBe("plan_save_failed");
  });
});
