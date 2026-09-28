import { describe, expect, it } from "vitest";
import { readTokenSnapshot } from "./readTokenSnapshot";

describe("token snapshot reads", () => {
  it("returns the two existing query results without changing their range or values", async () => {
    const entries = [{ totalTokens: 2 }];
    const daily = [{ totalTokens: 3 }];
    const result = await readTokenSnapshot(async () => entries, async () => daily);
    expect(result[0]).toBe(entries);
    expect(result[1]).toBe(daily);
  });

  it("waits for a slow sibling after either query fails", async () => {
    for (const failureFirst of [true, false]) {
      let finish!: (value: number) => void;
      let settled = false;
      const error = new Error("read failed");
      const slow = () => new Promise<number>((resolve) => { finish = resolve; });
      const fail = async () => { throw error; };
      const observed = readTokenSnapshot(failureFirst ? fail : slow, failureFirst ? slow : fail)
        .then(() => { throw new Error("unexpected success"); }, (reason) => { settled = true; return reason; });
      for (let i = 0; i < 5; i++) await Promise.resolve();
      expect(settled).toBe(false);
      finish(1);
      expect(await observed).toBe(error);
    }
  });

  it("also awaits both branches when a reader throws synchronously", async () => {
    let completed = false;
    const observed = readTokenSnapshot(() => { throw new Error("sync failure"); }, async () => { completed = true; return 2; });
    await expect(observed).rejects.toThrow("sync failure");
    expect(completed).toBe(true);
  });
});
