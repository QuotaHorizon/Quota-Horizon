import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { createAccountRefreshPolicy, createOverviewLoader, type AccountOverview } from "./accountOverview";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

function account(id: string, fetchedAt?: string, error?: string): AccountOverview {
  return { id, email: `${id}@example.com`, note: "", plan: "Pro", active: false, workPlan: null, usage: { fetchedAt, error } };
}

describe("account overview synchronization", () => {
  it("discards a read superseded by an add-account event and coalesces the burst", async () => {
    const old = deferred<string[]>();
    const fresh = deferred<string[]>();
    const seen: string[][] = [];
    let calls = 0;
    const loader = createOverviewLoader(() => (++calls === 1 ? old.promise : fresh.promise), (value) => seen.push(value), () => undefined);
    const pending = loader.reload();
    await Promise.resolve();
    void loader.reload();
    void loader.reload();
    old.resolve(["a"]);
    await Promise.resolve();
    expect(seen).toEqual([]);
    fresh.resolve(["a", "b"]);
    await pending;
    expect(calls).toBe(2);
    expect(seen).toEqual([["a", "b"]]);
  });

  it("retains existing rows on failure and accepts a later recovery", async () => {
    let rows = ["saved"];
    let failed = 0;
    let calls = 0;
    const loader = createOverviewLoader(async () => {
      if (++calls === 1) throw new Error("temporary read failure");
      return ["saved", "added"];
    }, (value) => { rows = value; }, () => { failed += 1; });
    await loader.reload();
    expect(rows).toEqual(["saved"]);
    expect(failed).toBe(1);
    await loader.reload();
    expect(rows).toEqual(["saved", "added"]);
  });

  it("does not restore a row or publish an error after disposal", async () => {
    const read = deferred<string[]>();
    const seen: string[][] = [];
    const loader = createOverviewLoader(() => read.promise, (value) => seen.push(value), () => { throw new Error("disposed callback"); });
    const pending = loader.reload();
    await Promise.resolve();
    loader.dispose();
    read.resolve(["deleted"]);
    await pending;
    expect(seen).toEqual([]);
  });

  it("grants the real popover only events, hide and read-only visibility permissions", () => {
    const capability = JSON.parse(readFileSync(new URL("../../../src-tauri/capabilities/capacity-popover.json", import.meta.url), "utf8"));
    expect(capability.windows).toEqual(["capacity-popover"]);
    expect(capability.permissions).toContain("core:event:allow-listen");
    expect(capability.permissions).toContain("core:event:allow-unlisten");
    expect(capability.permissions).toContain("core:window:allow-hide");
    expect(capability.permissions).toContain("core:window:allow-is-visible");
    expect(capability.permissions.every((value: string) => value.startsWith("core:event:") || ["core:window:allow-hide", "core:window:allow-is-visible"].includes(value))).toBe(true);
  });
});

describe("account overview background refresh policy", () => {
  const now = Date.parse("2026-09-06T10:00:00Z");
  const old = new Date(now - 120_000).toISOString();

  it("loads newly added accounts without a click, and old saved accounts when viewing", () => {
    const policy = createAccountRefreshPolicy();
    const rows = [account("new"), account("old", old), account("fresh", new Date(now).toISOString())];
    expect(policy.due(rows, now, false)).toEqual(["new"]);
    expect(policy.due(rows, now, true)).toEqual(["new", "old"]);
  });

  it("backs off failed accounts without hiding them or reacting to its own write event", () => {
    const policy = createAccountRefreshPolicy();
    const rows = [account("old", old, "HTTP 503")];
    policy.started(["old"], now);
    expect(policy.due(rows, now + 1, true)).toEqual([]);
    policy.completed("old", false, now);
    expect(policy.due(rows, now + 60_000, true)).toEqual([]);
    expect(policy.due(rows, now + 120_000, true)).toEqual(["old"]);
    expect(rows).toHaveLength(1);
    policy.reset();
    expect(policy.due(rows, now, true)).toEqual(["old"]);
  });

  it("caps backoff and never requests a broken local record automatically", () => {
    const policy = createAccountRefreshPolicy();
    for (let i = 0; i < 20; i += 1) policy.completed("old", false, now);
    const rows = [account("old", old), account("broken", old, "account_record_unreadable")];
    expect(policy.due(rows, now + 900_000 - 1, true)).toEqual([]);
    expect(policy.due(rows, now + 900_000, true)).toEqual(["old"]);
    policy.completed("old", true, now);
    expect(policy.due(rows, now + 60_000, true)).toEqual(["old"]);
  });
});
