import { describe, expect, it, vi } from "vitest";
import { readSearchBatches } from "./searchBatches";
import type { CodexThreadSearchResult } from "../../types";

const batch = (continuation: string | null): CodexThreadSearchResult => ({ entries: [], coverage: null, continuation });

describe("progressive session search", () => {
  it("reads one batch at a time until the native cursor is finished", async () => {
    const next = vi.fn().mockResolvedValueOnce(batch("second")).mockResolvedValueOnce(batch(null));
    const onBatch = vi.fn();
    expect(await readSearchBatches({ start: async () => batch("first"), next, isCurrent: () => true, onBatch })).toBe(true);
    expect(next.mock.calls).toEqual([["first"], ["second"]]);
    expect(onBatch).toHaveBeenCalledTimes(3);
  });

  it("drops a late batch and never continues a cancelled query", async () => {
    let resolve!: (result: CodexThreadSearchResult) => void;
    let current = true;
    const next = vi.fn();
    const onBatch = vi.fn();
    const pending = readSearchBatches({ start: () => new Promise((done) => { resolve = done; }), next, isCurrent: () => current, onBatch });
    current = false;
    resolve(batch("obsolete"));
    expect(await pending).toBe(false);
    expect(onBatch).not.toHaveBeenCalled();
    expect(next).not.toHaveBeenCalled();
  });

  it("keeps the last delivered batch if a later native read fails", async () => {
    const onBatch = vi.fn();
    await expect(readSearchBatches({ start: async () => batch("first"), next: async () => { throw new Error("expired"); }, isCurrent: () => true, onBatch })).rejects.toThrow("expired");
    expect(onBatch).toHaveBeenCalledOnce();
  });

  it("does not launch a continuation after the page closes while rendering a batch", async () => {
    let current = true;
    const next = vi.fn();
    expect(await readSearchBatches({ start: async () => batch("first"), next, isCurrent: () => current, onBatch: () => { current = false; } })).toBe(false);
    expect(next).not.toHaveBeenCalled();
  });
});
