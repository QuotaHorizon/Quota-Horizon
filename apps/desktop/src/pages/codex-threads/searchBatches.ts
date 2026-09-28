import type { CodexThreadSearchResult } from "../../types";

/** Each continuation is native-owned and one-use; never overlap batch reads.
 * The current-request check also prevents a closed/replaced search from
 * publishing a late response or starting another batch. */
export async function readSearchBatches({ start, next, isCurrent, onBatch }: {
  start: () => Promise<CodexThreadSearchResult>;
  next: (continuation: string) => Promise<CodexThreadSearchResult>;
  isCurrent: () => boolean;
  onBatch: (result: CodexThreadSearchResult) => void;
}): Promise<boolean> {
  let result = await start();
  while (isCurrent()) {
    onBatch(result);
    if (!result.continuation) return true;
    if (!isCurrent()) return false;
    result = await next(result.continuation);
  }
  return false;
}
