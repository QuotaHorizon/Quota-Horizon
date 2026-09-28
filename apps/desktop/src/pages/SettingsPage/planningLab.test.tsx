import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SettingsPage } from "./index";
import type { SettingsPageProps } from "../settings/types";

const { readSnapshot } = vi.hoisted(() => ({ readSnapshot: vi.fn() }));
vi.mock("../../api/backend", () => ({ getCapacityStatusSnapshot: readSnapshot }));
vi.mock("../settings/BasicSettingsCards", () => ({ AppearanceSettingsCards: () => null, PrivacySettingsCards: () => null, SystemSettingsCards: () => null }));
vi.mock("../settings/ConnectionSettingsCards", () => ({ CloudSettingsCard: () => null, WebProxySettingsCard: () => null }));
vi.mock("../settings/LocalSettingsCards", () => ({ RefreshSettingsCards: () => null, SecuritySettingsCard: () => null, StorageSettingsCards: () => null }));
vi.mock("../settings/UsageSettingsCards", () => ({ UsageSettingsCards: () => null }));
vi.mock("../settings/TotpSyncSettingsCard", () => ({ TotpSyncSettingsCard: () => null }));
vi.mock("../settings/NetworkProxySettings", () => ({ NetworkProxySettingsCard: () => null }));

beforeEach(() => readSnapshot.mockClear());
describe("planning lab placement", () => {
  it.each(["zh", "en"] as const)("places a closed, lazy lab after settings without reading quota in %s", (language) => {
    // Settings card implementations are outside this route/visibility contract.
    const settings = { language, t: (key: string) => key } as SettingsPageProps;
    const html = renderToStaticMarkup(<SettingsPage {...settings} />);
    const title = language === "zh" ? "规划实验室" : "Planning lab";
    expect(html.indexOf(title)).toBeGreaterThan(html.indexOf("settings-storage"));
    expect(html).not.toMatch(/<details[^>]*\bopen(?:[ =>])/);
    expect(html).not.toContain("近期节奏");
    expect(html).not.toContain("experimental pace estimates");
    expect(readSnapshot).not.toHaveBeenCalled();
  });
});
