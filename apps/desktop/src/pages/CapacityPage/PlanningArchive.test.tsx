import {renderToStaticMarkup} from "react-dom/server";
import {describe, expect, it, vi} from "vitest";
import type {DesktopPlanningArchiveEnvelope, DesktopDemandPlanEnvelope, PlanningArchiveEntry, PlanningArchiveQuery} from "../../../../capacity-preview/src/status";
import {archiveQueryKey, createPlanningArchiveStore} from "./planningArchiveStore";
import {ArchiveReadState, archiveReaderLabel, PlanningArchiveView} from "./PlanningArchiveView";
import {CapacityDemandView} from "./CapacityDemandView";
import {demandFromForm, localPlanInput} from "./demandPlanModel";
import {ActivityQuotaView} from "./ActivityQuotaView";

const query: PlanningArchiveQuery = {kind: "list", offset: 0};
const entry: PlanningArchiveEntry = {source: {kind: "plan", id: "private-plan"}, platform: "macos", architecture: "arm64", boundary: "native", codexVersion: "0.150.0-alpha.12.2", lastRecordedAt: "2026-09-26T04:00:00Z", planRevisions: 3, recordCount: 25, trialCount: 0};
function envelope(): DesktopPlanningArchiveEnvelope {return {historyContextId: "a", query, status: "available", reasonCode: "archive_available", result: {kind: "list", entries: [entry], hasMore: false}, generatedAt: "2026-09-26T04:00:00Z"};}
function deferred<T>() {let resolve!: (value: T) => void; const promise = new Promise<T>(yes => {resolve = yes;}); return {resolve, promise};}
describe("earlier reader archives", () => {
  it("keeps earlier scenario requests scoped to their resource and page",async()=>{
    const pace={kind:"pace" as const,source:{kind:"trial" as const,id:"old-trial"},offset:0};
    expect(archiveQueryKey(pace)).not.toBe(archiveQueryKey({...pace,offset:20}));
    expect(archiveQueryKey(pace)).not.toBe(archiveQueryKey({...pace,source:{...pace.source,id:"other-trial"}}));
    const read=vi.fn().mockResolvedValue({...envelope(),query:pace,result:{kind:"pace",trials:[],hasMore:false}});
    const store=createPlanningArchiveStore("a",pace,read);await store.reload();expect(store.getSnapshot().value?.result?.kind).toBe("pace");
    read.mockResolvedValueOnce({...envelope(),query:{...pace,offset:20},result:{kind:"pace",trials:[],hasMore:false}});await store.reload();expect(store.getSnapshot().value).toBeNull();
  });
  it("labels version and platform without exposing implementation identifiers", () => {
    expect(archiveReaderLabel(entry, "zh")).toBe("macOS · 本机读取器 · Codex 0.150.0-alpha.12.2");
    const suspicious = {...entry, platform: "some_internal_platform", codexVersion: "reader_file_hash", boundary: "unknown_boundary"};
    const label = archiveReaderLabel(suspicious, "zh");expect(label).not.toContain("_");expect(label).toContain("其他读取环境");
  });
  it("does not scan the archive until the user opens it, including the compact menu", () => {
    const renderArchive = vi.fn(() => null);
    const value: DesktopDemandPlanEnvelope = {schemaVersion: "1.0", historyContextId: "a", status: "available", reasonCode: "plan_available", plans: [], allowances: [], hasOtherEnvironmentPlans: true, generatedAt: "2026-09-26T04:00:00Z", observedAt: null, freshness: "not_applicable", decision: "not_assessed"};
    for (const compact of [true, false]) {
      const html = renderToStaticMarkup(<CapacityDemandView value={value} contextId="a" language="zh" compact={compact} onSave={async () => true} onRetry={() => undefined} renderArchive={renderArchive}/>);
      expect(html).toContain("旧读取环境的计划与记录");
    }
    expect(renderArchive).not.toHaveBeenCalled();
    const read = vi.fn(async () => envelope());
    renderToStaticMarkup(<PlanningArchiveView contextId="a" language="zh" sequence={0} read={read} canUse onUse={() => undefined}/>);
    expect(read).not.toHaveBeenCalled();
  });
  it("does not surface data after an account or binding change", () => {
    const child = <span>private old record</span>;
    for (const status of ["unavailable", "missing", "invalid_request"] as const) {
      const html = renderToStaticMarkup(<ArchiveReadState value={{...envelope(), status, result: null}} language="zh" failed={false} loading={false} onRetry={() => undefined}>{child}</ArchiveReadState>);
      expect(html).not.toContain("private old record");expect(html).not.toContain("archive_available");
    }
  });
  it("retains same-query facts on failure but disables plan reuse", async () => {
    const read = vi.fn().mockResolvedValueOnce(envelope()).mockRejectedValueOnce(new Error("private"));
    const store = createPlanningArchiveStore("a", query, read);await store.reload();await store.reload();
    expect(store.getSnapshot().value?.result?.kind).toBe("list");expect(store.getSnapshot().failed).toBe(true);
    const html = renderToStaticMarkup(<ArchiveReadState {...store.getSnapshot()} language="zh" onRetry={() => undefined}/>);
    expect(html).toContain("暂不能载入为新计划");expect(html).not.toContain("private");
  });
  it("clears data on denied access or missing anchors without retaining a usable old result", async () => {
    for (const status of ["unavailable", "missing"] as const) {
      const read = vi.fn().mockResolvedValueOnce(envelope()).mockResolvedValueOnce({...envelope(), status, result: null});
      const store = createPlanningArchiveStore("a", query, read);await store.reload();await store.reload();expect(store.getSnapshot().value?.result).toBeNull();
    }
  });
  it("rejects wrong account, page, record and result kind, not just a matching timestamp", async () => {
    for (const next of [{...envelope(), historyContextId: "b"}, {...envelope(), query: {kind: "list", offset: 20}}, {...envelope(), result: {kind: "quota", record: {}, comparison: {}}}]) {
      const store = createPlanningArchiveStore("a", query, async () => next as DesktopPlanningArchiveEnvelope);await store.reload();expect(store.getSnapshot().failed).toBe(true);expect(store.getSnapshot().value).toBeNull();
    }
    expect(archiveQueryKey({kind: "quota", source: entry.source, observationId: "one"})).not.toBe(archiveQueryKey({kind: "quota", source: entry.source, observationId: "two"}));
  });
  it("coalesces refreshes while mounted and stops trailing reads after closing", async () => {
    for (const close of [false, true]) {
      const pending = deferred<DesktopPlanningArchiveEnvelope>();const read = vi.fn().mockImplementationOnce(() => pending.promise).mockResolvedValue(envelope());
      const store = createPlanningArchiveStore("a", query, read);const dispose = store.subscribe(() => undefined);
      const task = store.reload();void store.reload();if (close) dispose();pending.resolve(envelope());await task;
      expect(read).toHaveBeenCalledTimes(close ? 1 : 2);dispose();
    }
  });
  it("keeps delayed old-reader results inside the old store", async () => {
    const pending = deferred<DesktopPlanningArchiveEnvelope>();const old = createPlanningArchiveStore("a", query, () => pending.promise);
    const next = createPlanningArchiveStore("b", query, async () => ({...envelope(), historyContextId: "b", result: {kind: "list", entries: [], hasMore: false}}));
    const task = old.reload();await next.reload();pending.resolve(envelope());await task;expect(next.getSnapshot().value?.historyContextId).toBe("b");expect(next.getSnapshot().value?.result).toMatchObject({entries: []});
  });
  it("preserves copied UTC deadlines during DST folds and rejects ambiguous manual input", () => {
    const previous = process.env.TZ;
    try {
      process.env.TZ = "America/New_York";
      const original = "2026-11-01T06:30:42Z", now = Date.parse("2026-10-31T00:00:00Z");
      expect(demandFromForm(localPlanInput(original), "active_hours", "2", now, original)?.horizonEnd).toBe("2026-11-01T06:30:42.000Z");
      expect(demandFromForm("2026-11-01T01:30", "active_hours", "2", now)).toBeNull();
      expect(demandFromForm(localPlanInput(original), "active_hours", "2", Date.parse(original), original)).toBeNull();
    } finally {if (previous === undefined) delete process.env.TZ; else process.env.TZ = previous;}
  });
  it("explains original-scope evidence without claiming that data was migrated", () => {
    const html = renderToStaticMarkup(<ActivityQuotaView archived language="zh" onRetry={() => undefined} value={{schemaVersion: "1.0", historyContextId: "a", observationId: "record", status: "available", reasonCode: "activity_evidence_available", record: null, comparison: {algorithmVersion: "activity-quota-comparison-v1", windows: [], queryTruncated: false}, generatedAt: "2026-09-26T04:00:00Z"}}/>);
    expect(html).toContain("原读取环境");expect(html).toContain("不混入当前读取器");
  });
});
