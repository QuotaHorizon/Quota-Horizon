import type { AppBuildInfo } from "../../../types";
import type { Language } from "../../../i18n";

export function buildIdentity(build: AppBuildInfo | undefined, language: Language) {
  const zh = language === "zh";
  const source = build?.commit ? `${build.commit.slice(0, 12)} · ${build.dirty === true
    ? (zh ? "含本地修改" : "local changes") : build.dirty === false ? (zh ? "干净构建" : "clean") : (zh ? "修改状态未知" : "change state unknown")}`
    : (zh ? "源码提交未记录" : "Source commit unavailable");
  const timestamp = build?.builtAtMs;
  const validTime = timestamp != null && Number.isFinite(timestamp) && timestamp > 0 && !Number.isNaN(new Date(timestamp).getTime());
  const builtAt = validTime ? new Date(timestamp).toISOString() : null;
  const platform = build ? `${({ macos: "macOS", windows: "Windows", linux: "Linux" }[build.platform] ?? build.platform)} · ${build.arch}` : null;
  return { source, builtAt, platform };
}

/** Explicit fields only: account paths and runtime configuration never enter the clipboard. */
export function buildInfoText(version: string, build: AppBuildInfo | undefined) {
  const identity = buildIdentity(build, "en");
  return [`QuotaHorizon ${version}`, `Source: ${identity.source}`, `Built: ${identity.builtAt ?? "unknown"}`,
    `Platform: ${identity.platform ?? "unknown"}`].join("\n");
}
