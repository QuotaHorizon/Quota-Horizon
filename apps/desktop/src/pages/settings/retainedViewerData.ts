import type { Language } from "../../i18n";
import type { LegacyMigrationArtifactKind, LegacyMigrationDryRunReport } from "../../types";

export interface RetainedDataView {
  kind: LegacyMigrationArtifactKind;
  title: string;
  status: "retained" | "missing" | "review";
  statusLabel: string;
  detail: string;
  next: string;
}

const kinds = ["quota_cache", "provider_mode", "restore_points"] as const;

export const retainedDataCopy = {
  zh: {
    title: "旧额度、代理与恢复记录如何处理",
    boundary: "迁移预览 · 尚未导入",
    retained: "保留在 Viewer", missing: "未发现记录", review: "需要复核",
    unavailable: "记录检查未完成，请查看源文件。",
    unreadableNext: "保留源目录，先在旧 Viewer 或备份中检查这部分数据，再重新预检。",
    titles: { quota_cache: "Viewer 旧额度快照", provider_mode: "旧第三方连接模式", restore_points: "Viewer 旧恢复记录" },
    missingDetails: {
      quota_cache: "未发现可迁移的 Viewer 旧快照；不影响 Horizon 获取新的额度。",
      provider_mode: "未找到旧模式记录；本步骤不会调整当前连接或网络。",
      restore_points: "未找到 Viewer 旧恢复记录。",
    },
    next: {
      quota_cache: "旧快照保留在 Viewer；在账号页刷新可获取新额度。",
      provider_mode: "在下方审阅 API 连接，导入后到连接页选择启用。不会自动切换，也不会替你退出旧模式。",
      restore_points: "旧恢复记录暂不能在 Horizon 中执行。Horizon 仅撤销自己建立并验证的恢复点；完整旧记录迁移仍待完成。",
    },
    quota: (count: number) => `检测到 ${count} 条旧快照，保留在源目录，不直接写入新的额度历史。`,
    provider: "检测到旧模式记录，但没有自动启用它或把旧状态视为 Horizon 的当前连接。",
    restores: (count: number) => `检测到 ${count} 个旧恢复记录。预检检查清单、路径与文件大小，尚未逐字节核验备份校验和，也未执行恢复。`,
    dependency: "旧模式引用的恢复记录缺失。这部分不能安全接管，当前账号与配置保持不动。",
    dependencyNext: "请先从 Viewer 或完整备份找回对应恢复记录，再重新检查。",
  },
  en: {
    title: "How old quota, proxy, and recovery records are handled",
    boundary: "Migration preview · Not imported",
    retained: "Kept in Viewer", missing: "No record found", review: "Needs review",
    unavailable: "Record inspection is incomplete. Check the source files.",
    unreadableNext: "Keep the source directory. Inspect these records in Viewer or a backup, then run preflight again.",
    titles: { quota_cache: "Old Viewer quota snapshots", provider_mode: "Old third-party connection mode", restore_points: "Old Viewer recovery records" },
    missingDetails: {
      quota_cache: "No old Viewer snapshots were found. Horizon can still fetch new quota observations.",
      provider_mode: "No old mode record was found. This step does not change the current connection or network.",
      restore_points: "No old Viewer recovery records were found.",
    },
    next: {
      quota_cache: "Old snapshots remain in Viewer. Refresh on Accounts to fetch current quota.",
      provider_mode: "Review API connections below, then choose one on Connections after import. Nothing is activated, and the old mode is not exited automatically.",
      restore_points: "Old recovery records cannot yet be executed in Horizon. Horizon only undoes changes from its own verified restore points; full legacy-record migration remains pending.",
    },
    quota: (count: number) => `${count} old snapshot(s) remain in the source directory and are not written directly into new quota history.`,
    provider: "An old mode record was found. It has not been activated or treated as Horizon’s current connection.",
    restores: (count: number) => `${count} old recovery record(s) found. Preflight checks manifests, paths, and file sizes, not every backup checksum. No restore has been executed.`,
    dependency: "The old mode references a missing recovery record. It cannot be taken over safely; the current account and configuration stay unchanged.",
    dependencyNext: "Recover the referenced record in Viewer or a complete backup, then inspect again.",
  },
} as const;

export function retainedDataViews(report: LegacyMigrationDryRunReport, language: Language): RetainedDataView[] {
  const copy = retainedDataCopy[language];
  return kinds.map((kind) => {
    const artifact = report.inventory.find((value) => value.kind === kind);
    const count = artifact?.recordCount;
    const validCount = typeof count === "number" && Number.isSafeInteger(count) && count >= 0;
    const dependencyMissing = kind === "provider_mode" && report.conflicts.some(
      (conflict) => conflict.kind === "provider_restore_point_missing" && conflict.count > 0,
    );
    if (dependencyMissing) return { kind, title: copy.titles[kind], status: "review", statusLabel: copy.review,
      detail: copy.dependency, next: copy.dependencyNext };
    if (artifact?.state === "missing" || (artifact?.state === "present" && validCount && count === 0)) {
      return { kind, title: copy.titles[kind], status: "missing", statusLabel: copy.missing,
        detail: copy.missingDetails[kind], next: copy.next[kind] };
    }
    if (artifact?.state !== "present" || !validCount) return { kind, title: copy.titles[kind],
      status: "review", statusLabel: copy.review, detail: copy.unavailable, next: copy.unreadableNext };
    return { kind, title: copy.titles[kind], status: "retained", statusLabel: copy.retained,
      detail: kind === "quota_cache" ? copy.quota(count)
        : kind === "restore_points" ? copy.restores(count) : copy.provider,
      next: copy.next[kind] };
  });
}
