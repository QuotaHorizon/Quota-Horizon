import type { UsageSummary } from "../../types";
import type { DesktopWorkPlanEnvelope } from "../../../../capacity-preview/src/status";

export interface AccountOverview {
  id: string;
  email: string;
  note: string;
  plan: string;
  active: boolean;
  usage: UsageSummary;
  workPlan: DesktopWorkPlanEnvelope | null;
}

export interface AccountOverviewEnvelope {
  schemaVersion: "1.0";
  privacyMode: boolean;
  accounts: AccountOverview[];
}

// Coalesce bursts, replay a change received during the read, and never publish
// a snapshot already superseded by an account event. Failures retain the UI.
export function createOverviewLoader<T>(
  read: () => Promise<T>,
  accept: (snapshot: T) => void,
  failed: () => void,
) {
  let revision = 0;
  let finishedRevision = 0;
  let pending: Promise<void> | null = null;
  let disposed = false;
  const reload = (): Promise<void> => {
    if (disposed) return Promise.resolve();
    revision += 1;
    if (pending) return pending;
    pending = Promise.resolve().then(async () => {
      while (!disposed) {
        const current = revision;
        try {
          const snapshot = await read();
          if (!disposed && current === revision) accept(snapshot);
        } catch {
          if (!disposed && current === revision) failed();
        }
        finishedRevision = current;
        if (current === revision) break;
      }
    }).finally(() => {
      pending = null;
      if (!disposed && finishedRevision !== revision) void reload();
    });
    return pending;
  };
  return { reload, dispose: () => { disposed = true; } };
}

export function createAccountRefreshPolicy() {
  const attempts = new Map<string, { retryAt: number; failures: number }>();
  return {
    due(accounts: AccountOverview[], now: number, includeStale: boolean) {
      return accounts.filter((account) => {
        const observed = Date.parse(account.usage.fetchedAt ?? "");
        const missing = !Number.isFinite(observed);
        const stale = !missing && now - observed >= 60_000;
        return (missing || (includeStale && stale))
          && now >= (attempts.get(account.id)?.retryAt ?? 0)
          && account.usage.error !== "account_record_unreadable";
      }).map(({ id }) => id);
    },
    started(ids: string[], now: number) {
      for (const id of ids) attempts.set(id, { retryAt: now + 60_000, failures: attempts.get(id)?.failures ?? 0 });
    },
    completed(id: string, success: boolean, now: number) {
      const failures = success ? 0 : (attempts.get(id)?.failures ?? 0) + 1;
      attempts.set(id, { failures, retryAt: now + Math.min(900_000, 60_000 * 2 ** failures) });
    },
    reset() { attempts.clear(); },
  };
}
