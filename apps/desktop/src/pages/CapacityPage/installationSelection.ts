import type { CodexCandidate, DesktopStatusEnvelope } from "../../../../capacity-preview/src/status";
import type { Language } from "../../i18n";

export function desktopInstallation(candidate: CodexCandidate) {
  return candidate.sources.some((source) => ["macos_chatgpt_bundle", "macos_bundle_registry"].includes(source))
    || /\/[^/]+\.app\/Contents\/Resources\/(?:codex|codex-cli\/(?:CodexCLI\.app\/Contents\/MacOS\/codex|bin\/codex))$/u.test(candidate.canonicalPath ?? "");
}

export function installationLabel(candidate: CodexCandidate, language: Language) {
  const desktop = desktopInstallation(candidate);
  const name = language === "zh"
    ? desktop ? "Codex 桌面 App · 内置读取组件" : "Codex CLI · 终端安装"
    : desktop ? "Codex desktop App · bundled reader" : "Codex CLI · terminal installation";
  const version = candidate.version?.replace(/^codex-cli\s+/u, "");
  return `${name}${desktop ? (language === "zh" ? "（推荐）" : " (recommended)") : ""}${version ? ` · ${version}` : ""}`;
}

export function selectableInstallation(candidate: CodexCandidate) {
  return candidate.verification === "verified" || candidate.verification === "confirmation_required";
}

/** Resolve against the latest envelope, including background discovery events.
 * A label/path can be unchanged while an update replaces the executable ID. */
export function currentInstallationChoice(envelope: DesktopStatusEnvelope | null, requestedId?: string) {
  const candidates = envelope?.candidates.filter(selectableInstallation) ?? [];
  return candidates.find((candidate) => candidate.executableId === requestedId)?.executableId
    ?? candidates.find((candidate) => candidate.executableId === envelope?.selectedExecutableId)?.executableId
    ?? candidates.find((candidate) => desktopInstallation(candidate) && candidate.verification === "verified")?.executableId
    ?? candidates.find(desktopInstallation)?.executableId
    ?? candidates.find((candidate) => candidate.verification === "verified")?.executableId
    ?? candidates[0]?.executableId;
}

export function installationIssueCode(error: unknown) {
  return error != null && typeof error === "object" && "code" in error && typeof error.code === "string"
    ? error.code : undefined;
}
