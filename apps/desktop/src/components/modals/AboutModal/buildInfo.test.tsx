import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { AboutModal } from ".";
import { buildIdentity, buildInfoText } from "./buildInfo";
import { translate } from "../../../i18n";
import type { AppBuildInfo } from "../../../types";

const build: AppBuildInfo = { commit: "0123456789abcdef0123456789abcdef01234567", dirty: true,
  builtAtMs: Date.parse("2026-10-30T01:02:03.456Z"), platform: "macos", arch: "aarch64" };

describe("installed build identity", () => {
  it("distinguishes dirty builds at the same version and commit by exact build time", () => {
    const first = buildInfoText("0.2.0", build);
    const second = buildInfoText("0.2.0", { ...build, builtAtMs: build.builtAtMs! + 1 });
    expect(first).not.toEqual(second);
    expect(first).toContain("local changes");
    expect(first).toContain(".456Z");
    expect(first).not.toContain("clean");
    expect(buildIdentity({ ...build, dirty: false }, "en").source).toContain("clean");
  });
  it("does not invent source identity for archived sources or old hosts", () => {
    expect(buildIdentity({ ...build, commit: null, dirty: null }, "en").source).toBe("Source commit unavailable");
    expect(buildInfoText("0.2.0", undefined)).toContain("Built: unknown");
    expect(buildInfoText("0.2.0", { ...build, builtAtMs: NaN })).toContain("Built: unknown");
  });
  it("copies only explicitly selected build fields", () => {
    const extra = { ...build, authPath: "/private/account", token: "sensitive-token" };
    const copied = buildInfoText("0.2.0", extra);
    expect(copied).not.toContain("private");
    expect(copied).not.toContain("sensitive-token");
    expect(copied).toContain("macOS · aarch64");
  });
  it.each(["zh", "en"] as const)("shows identity and a disabled manual check while checking in %s", language => {
    const html = renderToStaticMarkup(<AboutModal logoUrl="/icon.png" version="0.2.0" build={build}
      language={language} checkingUpdate onClose={() => {}} onFeedback={() => {}} onCheckUpdate={() => {}}
      checkUpdateLabel="Check" t={(key, values) => translate(language, key, values)} />);
    expect(html).toContain('aria-busy="true"');
    expect(html).toContain("disabled");
    expect(html).toContain(language === "zh" ? "含本地修改" : "local changes");
    expect(html).toContain(language === "zh" ? "复制版本信息" : "Copy version info");
  });
});
