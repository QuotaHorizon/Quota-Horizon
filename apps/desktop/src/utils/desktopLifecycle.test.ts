import { describe, expect, it } from "vitest";
import { isMenuBarResidentDesktop } from "./desktopLifecycle";

describe("menu-bar resident presentation", () => {
  it("applies only to a native macOS app", () => {
    expect(isMenuBarResidentDesktop(true, "MacIntel")).toBe(true);
    expect(isMenuBarResidentDesktop(true, "macOS")).toBe(true);
    expect(isMenuBarResidentDesktop(true, "Win32")).toBe(false);
    expect(isMenuBarResidentDesktop(true, "Linux x86_64")).toBe(false);
  });

  it("does not turn hosted web or browser preview into a resident app", () => {
    expect(isMenuBarResidentDesktop(false, "MacIntel")).toBe(false);
    expect(isMenuBarResidentDesktop(false, "")).toBe(false);
  });
});
