import type { UsageSummary } from "../types";

export const DEFAULT_ACCOUNT_REFRESH_CONCURRENCY = 3;

export interface AccountRefreshProgress {
  total: number;
  completed: number;
  succeeded: number;
  failed: number;
  activeIds: string[];
}

export interface AccountRefreshFailure {
  id: string;
  error: unknown;
  attempts: number;
}

export interface AccountRefreshResult {
  total: number;
  succeededIds: string[];
  failures: AccountRefreshFailure[];
}

interface AccountRefreshOptions {
  maxConcurrency?: number;
  maxAttempts?: number;
  retryDelayMs?: number;
  shouldRetry?: (error: unknown) => boolean;
  onProgress?: (progress: AccountRefreshProgress) => void;
}

function uniqueAccountIds(ids: string[]) {
  return [...new Set(ids.map((id) => id.trim()).filter(Boolean))];
}

function refreshErrorText(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}

export function isLikelyTransientUsageError(error: unknown) {
  const normalized = refreshErrorText(error).toLocaleLowerCase();
  if (/http\s+(401|402|403|404)\b/.test(normalized)) return false;
  return [
    "network",
    "timed out",
    "timeout",
    "connection",
    "dns",
    "tcp",
    "tls",
    "http 408",
    "http 425",
    "http 429",
    "http 500",
    "http 502",
    "http 503",
    "http 504",
    "cached usage retained",
    "请求超时",
    "连接失败",
    "网络错误",
    "保留了上次成功数据",
  ].some((fragment) => normalized.includes(fragment));
}

export function isFreshUsageRefresh(
  usage: UsageSummary,
  requestStartedAt: number,
  clockToleranceMs = 1_000,
) {
  if (!usage.fetchedAt || usage.error) return false;
  const fetchedAt = Date.parse(usage.fetchedAt);
  return Number.isFinite(fetchedAt) && fetchedAt >= requestStartedAt - clockToleranceMs;
}

function wait(milliseconds: number) {
  if (milliseconds <= 0) return Promise.resolve();
  return new Promise<void>((resolve) => {
    globalThis.setTimeout(resolve, milliseconds);
  });
}

export function createAccountRefreshSingleFlight(
  refresh: (id: string) => Promise<void>,
) {
  const active = new Map<string, Promise<void>>();
  return (id: string) => {
    const running = active.get(id);
    if (running) return running;
    const promise = refresh(id).finally(() => {
      if (active.get(id) === promise) active.delete(id);
    });
    active.set(id, promise);
    return promise;
  };
}

export async function runBoundedAccountRefresh(
  ids: string[],
  refresh: (id: string, attempt: number) => Promise<void>,
  options: AccountRefreshOptions = {},
): Promise<AccountRefreshResult> {
  const targets = uniqueAccountIds(ids);
  const maxConcurrency = Math.max(
    1,
    Math.min(targets.length || 1, Math.floor(options.maxConcurrency ?? DEFAULT_ACCOUNT_REFRESH_CONCURRENCY)),
  );
  const maxAttempts = Math.max(1, Math.floor(options.maxAttempts ?? 2));
  const retryDelayMs = Math.max(0, options.retryDelayMs ?? 350);
  const shouldRetry = options.shouldRetry ?? isLikelyTransientUsageError;
  const activeIds = new Set<string>();
  const succeededIds: string[] = [];
  const failures: AccountRefreshFailure[] = [];
  let completed = 0;
  let nextTarget = 0;

  const publishProgress = () => options.onProgress?.({
    total: targets.length,
    completed,
    succeeded: succeededIds.length,
    failed: failures.length,
    activeIds: [...activeIds],
  });

  publishProgress();

  const worker = async () => {
    while (nextTarget < targets.length) {
      const id = targets[nextTarget];
      nextTarget += 1;
      activeIds.add(id);
      publishProgress();

      let lastError: unknown;
      let attempts = 0;
      let succeeded = false;
      for (let attempt = 1; attempt <= maxAttempts; attempt += 1) {
        attempts = attempt;
        try {
          await refresh(id, attempt);
          succeededIds.push(id);
          succeeded = true;
          break;
        } catch (error) {
          lastError = error;
          if (attempt >= maxAttempts || !shouldRetry(error)) break;
          await wait(retryDelayMs);
        }
      }

      if (!succeeded) failures.push({ id, error: lastError, attempts });
      activeIds.delete(id);
      completed += 1;
      publishProgress();
    }
  };

  await Promise.all(Array.from({ length: maxConcurrency }, () => worker()));
  return { total: targets.length, succeededIds, failures };
}
