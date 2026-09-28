import type { Translate, TranslationKey } from "../i18n";
import type { LoginStatus } from "../types";

export function loginIsActive(status: LoginStatus | null | undefined): boolean {
  return status?.phase === "waiting" || status?.phase === "exchanging" || status?.phase === "browserFallback";
}

export function acceptLoginProgress(current: LoginStatus | null, next: LoginStatus): LoginStatus | null {
  if (!next.loginId || !next.phase) return current;
  if (!current) return next;
  if (current.loginId !== next.loginId) return current;
  if (!loginIsActive(current)) return current;
  // Opening a browser can finish after a fast callback has begun its exchange.
  if (current.phase === "exchanging" && (next.phase === "waiting" || next.phase === "browserFallback")) return current;
  return next;
}

const failureKeys: Record<string, TranslationKey> = {
  network: "login.error.network", denied: "login.error.denied",
  invalidResponse: "login.error.invalidResponse", storage: "login.error.storage",
  portBusy: "login.error.portBusy", openBrowser: "login.error.openBrowser",
  callbackUnavailable: "login.error.callbackUnavailable", alreadyRunning: "login.error.alreadyRunning",
};

export function loginProgressMessage(status: LoginStatus, t: Translate): string {
  if (!status.phase) return status.message;
  if (status.phase === "failed") return t(failureKeys[status.reason ?? ""] ?? "login.progress.failed");
  return t(`login.progress.${status.phase}`);
}
