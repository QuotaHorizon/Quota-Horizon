/** Visible-page change detection: metadata only until the catalog changes.
 * Mirrors CCHV's separation of change signals from list/detail refreshes, while
 * using existing IPC and filesystem primitives on all supported platforms.
 */
export function watchThreadChanges({ readRevision, refresh, canRead, onError, interval = 5_000 }: {
  readRevision: () => Promise<string>;
  refresh: () => Promise<boolean>;
  canRead: () => boolean;
  onError: () => void;
  interval?: number;
}) {
  let revision: string | null = null;
  let disposed = false;
  let running = false;
  const check = async (force = false) => {
    if (disposed || running || !canRead()) return;
    running = true;
    try {
      const next = await readRevision();
      if (disposed) return;
      if (force || revision == null || revision !== next) {
        if (await refresh() && !disposed) revision = next;
      }
    } catch {
      revision = null;
      if (!disposed) onError();
    } finally {
      running = false;
    }
  };
  const timer = setInterval(() => void check(), interval);
  return {
    check,
    dispose: () => { disposed = true; clearInterval(timer); },
  };
}
