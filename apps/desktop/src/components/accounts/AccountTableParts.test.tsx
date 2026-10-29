import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import type { Translate } from "../../i18n";
import { AccountPrivacyToggle, AccountResetCreditCount } from "./AccountTableParts";

const t = ((key: string) => key) as Translate;

describe("AccountPrivacyToggle", () => {
  it("renders an accessible control that reflects the current privacy state", () => {
    const markup = renderToStaticMarkup(
      <AccountPrivacyToggle enabled t={t} loading={false} onChange={() => undefined} />,
    );

    expect(markup).toContain('aria-label="table.showAccountDetails"');
    expect(markup).toContain('title="table.showAccountDetails"');
    expect(markup).toContain('aria-pressed="true"');
  });
});

describe("AccountResetCreditCount", () => {
  const state = { status: "loaded" as const, fetchedAt: "2026-10-30T00:00:00Z",
    data: { credits: [{ available: true, expiresAt: "2026-11-01T00:00:00Z" }] } };
  const now = Date.parse("2026-10-30T00:00:00Z");

  it.each(["zh", "en"] as const)("shows the captured expiry and opens existing detail without a row action in %s", (language) => {
    const onOpen = vi.fn();
    const stopPropagation = vi.fn();
    const element = AccountResetCreditCount({ state, language, onOpen, now })!;
    const markup = renderToStaticMarkup(element);
    expect(markup).toContain('type="button"');
    expect(markup).toContain(language === "zh" ? "1重置卡" : "1 reset card");
    expect(markup).toContain(language === "zh" ? "记录" : "Saved");
    expect(markup).toContain(language === "zh" ? "查看详情" : "View details");
    element.props.onClick({ stopPropagation });
    expect(stopPropagation).toHaveBeenCalledOnce();
    expect(onOpen).toHaveBeenCalledOnce();
  });

  it("keeps legacy availability unknown and omits absent or empty counts", () => {
    const old = { ...state, data: { credits: [{ expiresAt: "2026-11-01T00:00:00Z" }] } };
    expect(renderToStaticMarkup(<AccountResetCreditCount state={old} language="zh" onOpen={() => undefined} now={now} />))
      .toContain("到期时间未知");
    expect(AccountResetCreditCount({ language: "zh", onOpen: () => undefined, now })).toBeNull();
    expect(AccountResetCreditCount({ state: { ...state, data: { credits: [] } }, language: "zh", onOpen: () => undefined, now })).toBeNull();
  });
});
