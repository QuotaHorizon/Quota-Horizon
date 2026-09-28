import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { translate, type Translate } from "../i18n";
import type { LoginPhase, LoginStatus } from "../types";
import { LoginModal } from "../components/modals/LoginModal";
import { acceptLoginProgress, loginIsActive, loginProgressMessage } from "./loginProgress";

const status = (phase: LoginPhase, loginId = "current"): LoginStatus => ({ loginId, phase, ok: phase === "succeeded", message: "" });

describe("sign-in progress", () => {
  it("ignores the old attempt after another sign-in has started", () => {
    const current = status("waiting", "new");
    expect(acceptLoginProgress(current, status("succeeded", "old"))).toBe(current);
    expect(acceptLoginProgress(current, status("failed", "old"))).toBe(current);
  });

  it("does not revive a terminal result or announce completion twice", () => {
    for (const phase of ["cancelled", "succeeded", "timedOut", "failed"] as const) {
      const terminal = status(phase);
      expect(acceptLoginProgress(terminal, status("waiting"))).toBe(terminal);
      expect(acceptLoginProgress(terminal, status("succeeded"))).toBe(terminal);
      expect(loginIsActive(terminal)).toBe(false);
    }
  });

  it("keeps fallback pending and ignores late browser-open acknowledgements", () => {
    expect(loginIsActive(status("browserFallback"))).toBe(true);
    const exchange = status("exchanging");
    expect(acceptLoginProgress(exchange, status("browserFallback"))).toBe(exchange);
    expect(acceptLoginProgress(exchange, status("waiting"))).toBe(exchange);
  });

  it("adopts an existing attempt on remount without confusing legacy login events", () => {
    const pending = status("waiting");
    expect(acceptLoginProgress(null, pending)).toBe(pending);
    expect(acceptLoginProgress(pending, { ok: true, message: "legacy web sign-in" })).toBe(pending);
  });

  it("shows actionable bilingual messages without rendering unknown errors", () => {
    const bad = { ...status("failed"), reason: "SECRET_CANARY /private/file", message: "SECRET_CANARY" };
    expect(loginProgressMessage(bad, (key) => translate("zh", key))).toBe("登录未完成，请重试");
    expect(loginProgressMessage({ ...bad, reason: "portBusy" }, (key) => translate("en", key))).toContain("1455");
  });

  it("exposes cancellation while waiting and restores choices after cancellation", () => {
    const t: Translate = (key, values) => translate("zh", key, values);
    const render = (phase: LoginPhase) => renderToStaticMarkup(<LoginModal
      t={t} progress={status(phase)} onClose={() => {}} onStart={() => {}}
      onWebSession={() => {}} onImport={() => {}} onImportClipboard={() => {}}
      onCancelLogin={() => {}}
    />);
    const pending = render("waiting");
    expect(pending).toContain("取消本次登录");
    expect(pending).toContain("切换当前 Codex 登录仍需单独预览并确认");
    expect(pending).not.toContain("在应用内登录</b>");
    const cancelled = render("cancelled");
    expect(cancelled).toContain("已取消本次登录");
    expect(cancelled).toContain("在应用内登录</b>");
  });
});
