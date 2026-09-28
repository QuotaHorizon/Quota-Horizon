import type { DesktopHistoryEnvelope } from "../../../../capacity-preview/src/status";
import { chronologicalHistoryPoints } from "./presentation";

export interface HistoryView {
  key: string;
  history: DesktopHistoryEnvelope | null;
  issue: string | null;
}

export function visibleHistory(view: HistoryView, key: string): HistoryView {
  return view.key === key ? view : { key, history: null, issue: null };
}

export function settleHistory(view: HistoryView, key: string, contextId: string | undefined, incoming: DesktopHistoryEnvelope): HistoryView {
  const current = visibleHistory(view, key);
  // The native query and status must belong to the same account/environment.
  // A binding may have changed while the IPC request was in flight.
  if (contextId && incoming.historyContextId !== contextId) {
    return { key, history: null, issue: "history_context_unavailable" };
  }
  const transient = incoming.status === "failed" || (incoming.status === "unavailable"
    && incoming.reasonCode === "history_account_unavailable");
  if (transient && current.history?.status === "available") {
    return { ...current, issue: incoming.reasonCode };
  }
  const next = { ...incoming, points: chronologicalHistoryPoints(incoming.points) };
  // Duplicate status events need not repaint an unchanged chart.
  const history = JSON.stringify(next) === JSON.stringify(current.history) ? current.history : next;
  return { key, history, issue: null };
}
