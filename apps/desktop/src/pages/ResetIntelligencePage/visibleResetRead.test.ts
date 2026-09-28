import { afterEach, describe, expect, it, vi } from "vitest";
import { observeResetRead, postReadReceipts } from "./visibleResetRead";
import type { PublicPost, PublicResetTimeline, PublicRevisionChange } from "./types";

afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); });

function viewport() {
  vi.useFakeTimers();
  const win = Object.assign(new EventTarget(), { innerHeight: 800 });
  const doc = Object.assign(new EventTarget(), { defaultView: win, visibilityState: "visible", hasFocus: (): boolean => true });
  let notify!: (entries: Partial<IntersectionObserverEntry>[]) => void;
  const disconnect = vi.fn();
  vi.stubGlobal("IntersectionObserver", class {
    constructor(callback: typeof notify) { notify = callback; }
    observe() {}
    disconnect = disconnect;
  });
  const seen = vi.fn();
  const stop = observeResetRead({ ownerDocument: doc } as unknown as HTMLElement, seen);
  const show = (ratio = 1) => notify([{ isIntersecting: ratio > 0, intersectionRatio: ratio, intersectionRect: { height: 300 * ratio } as DOMRectReadOnly }]);
  return { win, doc, seen, stop, show, disconnect };
}

describe("visible reset reading", () => {
  it("requires visible dwell and only acknowledges once", () => {
    const view = viewport();
    view.show(); vi.advanceTimersByTime(1199); expect(view.seen).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1); expect(view.seen).toHaveBeenCalledTimes(1);
    view.show(); vi.advanceTimersByTime(5000); expect(view.seen).toHaveBeenCalledTimes(1);
    view.stop(); expect(view.disconnect).toHaveBeenCalledOnce();
  });
  it("does not acknowledge unseen, briefly seen, collapsed or unmounted content", () => {
    const view = viewport();
    vi.advanceTimersByTime(5000); expect(view.seen).not.toHaveBeenCalled();
    view.show(); vi.advanceTimersByTime(1000); view.show(0); vi.advanceTimersByTime(5000);
    expect(view.seen).not.toHaveBeenCalled();
    view.show(); view.stop(); vi.advanceTimersByTime(5000); expect(view.seen).not.toHaveBeenCalled();
  });
  it("restarts dwell after hidden/background windows regain focus", () => {
    const view = viewport();
    view.doc.hasFocus = () => false; view.show(); vi.advanceTimersByTime(5000);
    expect(view.seen).not.toHaveBeenCalled();
    view.doc.hasFocus = () => true; view.win.dispatchEvent(new Event("focus")); vi.advanceTimersByTime(800);
    view.doc.visibilityState = "hidden"; view.doc.dispatchEvent(new Event("visibilitychange")); vi.advanceTimersByTime(5000);
    expect(view.seen).not.toHaveBeenCalled();
    view.doc.visibilityState = "visible"; view.doc.dispatchEvent(new Event("visibilitychange")); vi.advanceTimersByTime(1200);
    expect(view.seen).toHaveBeenCalledOnce(); view.stop();
  });
  it("cancels a pending dwell on blur", () => {
    const view = viewport(); view.show(); vi.advanceTimersByTime(800);
    view.win.dispatchEvent(new Event("blur")); vi.advanceTimersByTime(5000);
    expect(view.seen).not.toHaveBeenCalled(); view.stop();
  });
  it("matches only the displayed post body and latest exact public revision", () => {
    const post = { id: "123", url: "https://x.com/thsottiaux/status/123", text: "Reset next week." } as PublicPost;
    const current = { signalId: "a", revision: 1, summary: post.text, source: { canonicalUrl: "https://twitter.com/thsottiaux/status/123?x=1" } } as PublicRevisionChange["current"];
    const data: PublicResetTimeline = { sources: [], entries: [], revisionCount: 1, evidenceFamilyCount: 1,
      changes: { totalCount: 1, items: [{ previous: null, current, unread: true }] } };
    expect(postReadReceipts(data, post)).toEqual([{ signalId: "a", revision: 1 }]);
    const newer = { ...current, revision: 2, summary: "Correction: next month." };
    data.changes!.items.push({ previous: current, current: newer, unread: true });
    expect(postReadReceipts(data, post)).toEqual([]);
    expect(postReadReceipts(data, { ...post, text: newer.summary })).toEqual([{ signalId: "a", revision: 2 }]);
    expect(postReadReceipts(data, { ...post, url: "https://example.org/123" })).toEqual([]);
  });
});
