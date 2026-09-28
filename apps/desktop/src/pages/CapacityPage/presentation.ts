import type {
  DesktopHistoryPoint,
  DesktopHistoryEnvelope,
  DesktopQuotaWindow,
} from "../../../../capacity-preview/src/status";
import type { Language } from "../../i18n";

export function historyEmptyMessage(history: DesktopHistoryEnvelope | null, language: Language) {
  const zh = language === "zh";
  if (!history) return zh ? "正在读取本地历史…" : "Loading local history…";
  if (history.status === "available") return zh
    ? "本地历史已就绪，下一次成功读取额度后将显示首个样本。"
    : "Local history is ready. The next successful quota read will add the first sample.";
  switch (history.reasonCode) {
    case "history_keychain_denied":
    case "history_keychain_interaction_required":
      return zh ? "授权后可查看旧历史并继续记录额度。" : "Authorize to view saved history and resume recording.";
    case "history_keychain_missing_entitlement":
      return zh ? "当前安装无法访问历史密钥，请检查安装版本。" : "This installation cannot access the history key. Check the installed version.";
    case "history_keychain_corrupt":
      return zh ? "历史密钥异常，暂时无法读取。请保留数据并查看问题排查。" : "The history key is invalid. Keep your data and open troubleshooting.";
    case "history_keychain_unavailable":
      return zh ? "钥匙串暂不可用，历史记录已暂停。" : "Keychain is unavailable. History recording is paused.";
    case "history_account_unavailable":
    case "history_context_unavailable":
      return zh ? "等待当前账号的首次额度读取。" : "Waiting for this account’s first quota read.";
    default:
      return history.status === "binding_required"
        ? (zh ? "暂时无法识别账号，历史记录已暂停。" : "The account could not be identified. History recording is paused.")
        : (zh ? "历史读取失败，请重试。" : "History could not be loaded. Try again.");
  }
}

export function chronologicalHistoryPoints(points: DesktopHistoryPoint[]) {
  return [...points].sort((left, right) => Date.parse(left.capturedAt) - Date.parse(right.capturedAt));
}

export function buildHistoryCoordinates(points: DesktopHistoryPoint[], width = 640, height = 180, padding = 12) {
  const valid = chronologicalHistoryPoints(points).filter((point) => Number.isFinite(point.remainingPercent));
  const start = Date.parse(valid[0]?.capturedAt ?? "");
  const end = Date.parse(valid[valid.length - 1]?.capturedAt ?? "");
  return valid.map((point, index) => {
    const timestamp = Date.parse(point.capturedAt);
    const progress = valid.length === 1 ? 0.5
      : Number.isFinite(start) && Number.isFinite(end) && end > start && Number.isFinite(timestamp)
        ? Math.max(0, Math.min(1, (timestamp - start) / (end - start)))
        : index / (valid.length - 1);
    return {
      x: padding + progress * Math.max(1, width - padding * 2),
      y: padding + (100 - clampPercent(point.remainingPercent)) / 100 * Math.max(1, height - padding * 2),
      remainingPercent: clampPercent(point.remainingPercent),
      capturedAt: point.capturedAt,
      // Break at a changed window boundary; this is not a claimed official reset.
      newCycle: index > 0 && point.resetsAt !== valid[index - 1].resetsAt,
    };
  });
}

export function clampPercent(value: number) {
  return Math.min(100, Math.max(0, Number.isFinite(value) ? value : 0));
}

export function selectPrimaryQuotaWindow(windows: DesktopQuotaWindow[]) {
  return accountQuotaWindows(windows).sort((left, right) => (
    (right.windowMinutes ?? -1) - (left.windowMinutes ?? -1)
  ))[0] ?? null;
}

export function selectShortestQuotaWindow(windows: DesktopQuotaWindow[]) {
  return accountQuotaWindows(windows).sort((left, right) => (
    (left.windowMinutes ?? Number.MAX_SAFE_INTEGER)
      - (right.windowMinutes ?? Number.MAX_SAFE_INTEGER)
  ))[0] ?? null;
}

function accountQuotaWindows(windows: DesktopQuotaWindow[]) {
  const canonical = windows.filter((window) => (
    window.limitId === "codex" || window.limitId.startsWith("codex:")
  ));
  return canonical.length ? canonical : [...windows];
}

export function buildHistoryPath(
  points: DesktopHistoryPoint[],
  width = 640,
  height = 180,
  padding = 12,
) {
  if (!points.length) return "";
  const innerWidth = Math.max(1, width - padding * 2);
  const innerHeight = Math.max(1, height - padding * 2);
  return points.map((point, index) => {
    const progress = points.length === 1 ? 0.5 : index / (points.length - 1);
    const x = padding + progress * innerWidth;
    const y = padding + (100 - clampPercent(point.remainingPercent)) / 100 * innerHeight;
    return `${index === 0 ? "M" : "L"}${x.toFixed(1)},${y.toFixed(1)}`;
  }).join(" ");
}
