import { Alert, Button, Progress } from "antd";
import type { Language } from "../../i18n";
import type { CodexThreadSearchCoverage } from "../../types";

export function searchCoverageCopy(coverage: CodexThreadSearchCoverage, matches: number, language: Language) {
  const zh = language === "zh";
  const incomplete = coverage.incompleteSessions > 0 || coverage.pendingSessions > 0;
  return {
    type: incomplete ? "warning" as const : "info" as const,
    title: coverage.pendingSessions > 0
      ? (zh ? `找到 ${matches} 条会话 · ${coverage.pendingSessions} 条尚未搜索完` : `${matches} matches · ${coverage.pendingSessions} sessions still pending`)
      : incomplete
      ? (zh ? `找到 ${matches} 条会话 · ${coverage.incompleteSessions} 条未完整搜索` : `${matches} matches · ${coverage.incompleteSessions} sessions not fully searched`)
      : (zh ? `找到 ${matches} 条会话 · 已包含关联的历史分段` : `${matches} matches · Linked history segments included`),
    description: incomplete
      ? (zh ? `共 ${coverage.totalSessions} 条，已读取 ${coverage.searchedSessions} 条。部分文件变动、缺失或超过读取上限。${coverage.skippedRecords > 0 ? `有 ${coverage.skippedRecords} 条过大或不完整记录未参与搜索。` : ""}`
        : `${coverage.totalSessions} sessions; ${coverage.searchedSessions} read. Some files changed, are missing or exceed the read limit.${coverage.skippedRecords > 0 ? ` ${coverage.skippedRecords} oversized or incomplete records were skipped.` : ""}`)
      : (zh ? "匹配标题、目录或正文，内容仅在本机读取；结果片段标出原消息时间。" : "Matches titles, folders, or content, read only on this device. Excerpts show the original message time."),
  };
}

export function searchEmptyCopy(incomplete: boolean, language: Language) {
  return incomplete
    ? (language === "zh" ? "已检索的内容中没有匹配；部分会话尚未完整搜索。" : "No match in the checked content; some sessions were not fully searched.")
    : (language === "zh" ? "没有匹配的会话" : "No matching sessions");
}

export function SearchCoverage({ coverage, matches, language, searching = false, stale = false, stopped = false, onStop, onRestart }: {
  coverage: CodexThreadSearchCoverage | null;
  matches: number;
  language: Language;
  searching?: boolean;
  stale?: boolean;
  stopped?: boolean;
  onStop?: () => void;
  onRestart?: () => void;
}) {
  const zh = language === "zh";
  if (searching) return <Alert showIcon type="info"
    message={coverage ? (zh ? `正在搜索 · 已检查 ${coverage.completedSessions} / ${coverage.totalSessions} 条会话 · 找到 ${matches} 条` : `Searching · ${coverage.completedSessions} / ${coverage.totalSessions} sessions checked · ${matches} matches`)
      : (zh ? "正在搜索本地会话…" : "Searching local sessions…")}
    action={onStop && <Button size="small" onClick={onStop}>{zh ? "停止搜索" : "Stop search"}</Button>}
    description={coverage ? <>
      <Progress percent={coverage.totalSessions ? Math.floor(100 * coverage.completedSessions / coverage.totalSessions) : 0} size="small" />
      {zh ? `已读取 ${(coverage.decodedBytes / 1048576).toFixed(1)} MiB。分批继续读取，结果逐步补齐；可以继续输入或停止。` : `${(coverage.decodedBytes / 1048576).toFixed(1)} MiB read. Batches continue from the saved position; you can keep typing or stop.`}
    </> : (zh ? "搜索完成前保留上次结果，可以继续输入。" : "Previous results stay visible while searching. You can keep typing.")} />;
  if (stopped) return <Alert showIcon type="warning" message={zh ? "搜索已停止，保留已找到的结果" : "Search stopped; confirmed results kept"}
    description={zh ? "部分内容尚未搜索，可重新搜索。" : "Some content remains unsearched. Search again to continue."}
    action={onRestart && <Button size="small" onClick={onRestart}>{zh ? "重新搜索" : "Search again"}</Button>} />;
  if (!coverage) return null;
  const copy = searchCoverageCopy(coverage, matches, language);
  return <Alert showIcon type={copy.type} message={copy.title}
    description={<>{copy.description}{stale && <div>{zh ? "会话有更新，当前搜索结果保持不变；刷新搜索可纳入新内容。" : "Sessions changed. Current results are preserved; refresh the search to include new content."}</div>}</>}
    action={stale && onRestart && <Button size="small" onClick={onRestart}>{zh ? "刷新搜索" : "Refresh search"}</Button>} />;
}
