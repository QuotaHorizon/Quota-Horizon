import { describe, expect, it, vi } from "vitest";
import { watchPageVisibility } from "./watchPageVisibility";

const flush = async () => { for (let i = 0; i < 6; i++) await Promise.resolve(); };
function fixture() {
  let shown = true;
  const documentTarget = new EventTarget();
  const windowTarget = new EventTarget();
  const onVisible = vi.fn();
  return { documentTarget, windowTarget, onVisible, documentVisible: () => shown,
    setDocumentVisible(value: boolean) { shown = value; documentTarget.dispatchEvent(new Event("visibilitychange")); },
  };
}

describe("page and native-window visibility", () => {
  it("publishes DOM hide/show changes but does not treat ordinary blur as hiding", () => {
    const f = fixture();
    const stop = watchPageVisibility(f);
    f.windowTarget.dispatchEvent(new Event("blur"));
    expect(f.onVisible.mock.calls).toEqual([[true]]);
    f.setDocumentVisible(false);
    f.setDocumentVisible(true);
    expect(f.onVisible.mock.calls).toEqual([[true], [false], [true]]);
    stop();
    f.setDocumentVisible(false);
    expect(f.onVisible).toHaveBeenCalledTimes(3);
  });

  it("checks the native window when DOM visibility stays true", async () => {
    const f = fixture();
    let visible = false;
    let nativeChanged!: () => void;
    const unlisten = vi.fn();
    const stop = watchPageVisibility({ ...f, native: {
      read: async () => visible,
      subscribe: (changed) => { nativeChanged = changed; return [Promise.resolve(unlisten)]; },
    } });
    await flush();
    expect(f.onVisible).toHaveBeenLastCalledWith(false);
    visible = true;
    nativeChanged();
    await flush();
    expect(f.onVisible).toHaveBeenLastCalledWith(true);
    visible = false;
    nativeChanged();
    await flush();
    expect(f.onVisible).toHaveBeenLastCalledWith(false);
    stop(); stop();
    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it("ignores an earlier slow native result after a newer event", async () => {
    const f = fixture();
    const replies: ((value: boolean) => void)[] = [];
    const stop = watchPageVisibility({ ...f, native: {
      read: () => new Promise((resolve) => replies.push(resolve)), subscribe: () => [],
    } });
    f.windowTarget.dispatchEvent(new Event("blur"));
    replies[1](false);
    await flush();
    replies[0](true);
    await flush();
    expect(f.onVisible.mock.calls).toEqual([[false]]);
    stop();
  });

  it("does not allow an in-flight native read to reopen a hidden document", async () => {
    const f = fixture();
    let resolve!: (value: boolean) => void;
    const stop = watchPageVisibility({ ...f, native: {
      read: () => new Promise((yes) => { resolve = yes; }), subscribe: () => [],
    } });
    f.setDocumentVisible(false);
    resolve(true);
    await flush();
    expect(f.onVisible.mock.calls).toEqual([[false]]);
    stop();
  });

  it("unsubscribes late registrations exactly once, without late reads or state writes", async () => {
    const f = fixture();
    let complete!: (unlisten: () => void) => void;
    const unlisten = vi.fn();
    const read = vi.fn(async () => true);
    const stop = watchPageVisibility({ ...f, native: {
      read, subscribe: () => [new Promise((resolve) => { complete = resolve; })],
    } });
    stop(); stop();
    complete(unlisten);
    await flush();
    expect(unlisten).toHaveBeenCalledTimes(1);
    expect(read).toHaveBeenCalledTimes(1);
    expect(f.onVisible).not.toHaveBeenCalled();
  });

  it("falls back to DOM events if native IPC or listener registration is unavailable", async () => {
    const f = fixture();
    const stop = watchPageVisibility({ ...f, native: {
      read: async () => { throw new Error("unsupported"); },
      subscribe: () => [Promise.reject(new Error("unsupported"))],
    } });
    await flush();
    expect(f.onVisible).toHaveBeenLastCalledWith(true);
    f.setDocumentVisible(false);
    expect(f.onVisible).toHaveBeenLastCalledWith(false);
    stop();
  });
});
