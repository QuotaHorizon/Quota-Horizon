import type { CodexThreadDetailPage } from "../../types";

export function mergeThreadDetailPage(current: CodexThreadDetailPage, incoming: CodexThreadDetailPage, earlier: boolean) {
  if (current.summary.sessionId !== incoming.summary.sessionId || current.revision !== incoming.revision) return current;
  const ordered = earlier ? [...incoming.items, ...current.items] : [...current.items, ...incoming.items];
  const seen = new Set<string>();
  const items = ordered.filter((item) => {
    if (seen.has(item.id)) return false;
    seen.add(item.id);
    return true;
  });
  return {
    ...current,
    items,
    previousOffset: earlier ? incoming.previousOffset : current.previousOffset,
    offset: earlier ? incoming.offset : current.offset,
    nextOffset: earlier ? current.nextOffset : incoming.nextOffset,
  };
}
