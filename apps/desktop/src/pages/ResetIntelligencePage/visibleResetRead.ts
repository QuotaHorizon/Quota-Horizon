import { useEffect, useRef } from "react";
import type { PublicPost, PublicReadReceipt, PublicResetTimeline } from "./types";

const normalizeText = (text: string) => text.replace(/\s+/gu, " ").trim();
const postIdentity = (url: string) => url.match(/^https:\/\/(?:www\.)?(?:x\.com|twitter\.com)\/thsottiaux\/status\/(\d+)(?:[/?#]|$)/iu)?.[1];

/** A displayed post can acknowledge its matching body, not a newer ledger
 * correction or an unrelated record that happens to share its source. */
export function postReadReceipts(timeline: PublicResetTimeline | null, post: PublicPost): PublicReadReceipt[] {
  const id = postIdentity(post.url);
  if (!id) return [];
  const latest = new Map<string, number>();
  for (const { signal } of timeline?.entries ?? []) latest.set(signal.signalId, Math.max(latest.get(signal.signalId) ?? 0, signal.revision));
  for (const { current } of timeline?.changes?.items ?? []) latest.set(current.signalId, Math.max(latest.get(current.signalId) ?? 0, current.revision));
  return (timeline?.changes?.items ?? []).filter(({ current, unread }) => unread
    && current.revision === latest.get(current.signalId) && postIdentity(current.source.canonicalUrl) === id
    && normalizeText(current.summary) === normalizeText(post.text))
    .map(({ current }) => ({ signalId: current.signalId, revision: current.revision }));
}

/** Only dwell in a visible, focused window counts. Collapsed or off-screen
 * content, leaving the page, and a changing revision all cancel the timer. */
export function observeResetRead(element: HTMLElement, onSeen: () => void) {
  const doc = element.ownerDocument;
  const win = doc.defaultView;
  if (!win || typeof IntersectionObserver === "undefined") return () => {};
  let visible = false;
  let done = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const cancel = () => { clearTimeout(timer); timer = undefined; };
  const check = () => {
    cancel();
    if (!done && visible && doc.visibilityState === "visible" && doc.hasFocus()) {
      timer = setTimeout(() => {
        timer = undefined;
        if (visible && doc.visibilityState === "visible" && doc.hasFocus()) { done = true; onSeen(); }
      }, 1200);
    }
  };
  const observer = new IntersectionObserver(([entry]) => {
    visible = entry.isIntersecting && (entry.intersectionRatio >= .5
      || entry.intersectionRect.height >= win.innerHeight * .6);
    check();
  }, { threshold: [0, .25, .5, .75, 1] });
  observer.observe(element);
  doc.addEventListener("visibilitychange", check);
  win.addEventListener("focus", check);
  win.addEventListener("blur", cancel);
  return () => {
    cancel(); observer.disconnect(); doc.removeEventListener("visibilitychange", check);
    win.removeEventListener("focus", check); win.removeEventListener("blur", cancel);
  };
}

export function useVisibleResetRead(receipts: PublicReadReceipt[], onRead?: (receipts: PublicReadReceipt[]) => void) {
  const ref = useRef<HTMLDivElement>(null);
  const callback = useRef(onRead);
  callback.current = onRead;
  const key = JSON.stringify(receipts);
  const enabled = !!onRead && receipts.length > 0;
  useEffect(() => {
    if (!enabled || !ref.current) return;
    const shown = JSON.parse(key) as PublicReadReceipt[];
    return observeResetRead(ref.current, () => callback.current?.(shown));
  }, [key, enabled]);
  return ref;
}
