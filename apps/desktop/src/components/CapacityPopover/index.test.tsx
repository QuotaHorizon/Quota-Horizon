import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";
import { CapacityPopover } from "./index";

const settings = vi.hoisted(() => ({ language: "zh" as "zh" | "en", native: false }));
vi.mock("../../hooks/useLanguage", () => ({ useLanguage: () => settings }));
vi.mock("../../api/backend", () => ({
  get hasLocalBackend() { return settings.native; },
  getCapacityStatusSnapshot: () => null,
}));

function render(mode: string) {
  vi.stubGlobal("window", { location: { search: `?preview=${mode}` } });
  return renderToStaticMarkup(<CapacityPopover />);
}
afterEach(() => { vi.unstubAllGlobals(); settings.language = "zh"; settings.native = false; });

describe("quota popover uses localized notices in both content and empty states", () => {
  it("renders the real cache-only reason as a neutral notice with quota and age intact", () => {
    const markup = render("cached");
    expect(markup).toContain("上次保存的额度");
    expect(markup).toContain('data-kind="info"');
    expect(markup).toContain("72%");
    expect(markup).toContain("42 分钟前更新");
    expect(markup).not.toContain("managed_quota_only");
    expect(markup).not.toContain("triangle-alert");
  });

  it("localizes a refresh failure without using the raw backend message", () => {
    const markup = render("stale");
    expect(markup).toContain('data-kind="warning"');
    expect(markup).toContain("读取 Codex 额度超时");
    expect(markup).toContain("上次保存的额度");
    expect(markup).not.toContain("Showing the last trusted snapshot");
    expect(markup).not.toContain("app_server_timeout");
  });

  it("also localizes the no-quota failure instead of exposing issue.message", () => {
    const markup = render("error");
    expect(markup).toContain("读取 Codex 额度超时");
    expect(markup).not.toContain("capacity service");
    expect(markup).not.toContain("上次保存的额度");
    expect(markup).not.toContain("app_server_timeout");
  });

  it("has English copy and removes the notice when live quota is complete", () => {
    settings.language = "en";
    expect(render("cached")).toContain("Showing the last saved quota");
    expect(render("error")).toContain("Reading Codex quota timed out");
    expect(render("ready")).not.toContain('role="status"');
  });

  it("never loads visual-fixture quota or failures in the native application", () => {
    settings.native = true;
    for (const mode of ["ready", "cached", "stale", "error"]) {
      const markup = render(mode);
      expect(markup).toContain("正在读取 Codex 额度");
      expect(markup).not.toContain("72%");
      expect(markup).not.toContain("读取 Codex 额度超时");
    }
  });
});
