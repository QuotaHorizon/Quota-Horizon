import { describe, expect, it } from "vitest";
import type { Translate } from "../../i18n";
import { buildDashboardMenuItems } from "./dashboardMenuItems";

const t = ((key: string) => key) as Translate;

describe("dashboard lifecycle actions", () => {
  it("exposes reset intelligence in navigation and command search", () => {
    const menu = buildDashboardMenuItems(t, false, true);
    expect(menu.navigate?.some((item) => item && "key" in item && item.key === "reset-intelligence")).toBe(true);
    expect(menu.search.some((item) => item.id === "reset-intelligence")).toBe(true);
  });

  it("offers return-to-menu-bar instead of quit in both macOS action surfaces", () => {
    const menu = buildDashboardMenuItems(t, false, true);
    const keys = menu.file?.map((item) => item && "key" in item ? item.key : null);
    expect(keys).toContain("hide-main-window");
    expect(keys).not.toContain("quit-app");
    expect(menu.search.some((item) => item.id === "hide-main-window")).toBe(true);
    expect(menu.search.some((item) => item.id === "quit-app")).toBe(false);
    expect(keys).toContain("restart-app");
  });

  it("preserves the existing quit action on other platforms", () => {
    const menu = buildDashboardMenuItems(t, true, false);
    expect(menu.file?.some((item) => item && "key" in item && item.key === "quit-app")).toBe(true);
    expect(menu.search.some((item) => item.id === "quit-app")).toBe(true);
    expect(menu.search.some((item) => item.id === "hide-main-window")).toBe(false);
  });
});
