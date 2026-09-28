import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { Alert, Button, Drawer, Modal, Spin, Tag } from "antd";
import { Bot, Copy, FolderCog, FolderOpen, Play, Terminal, User, Wrench } from "lucide-react";
import {
  chooseCodexThreadDirectory, confirmCodexThreadRebind, loadCodexThreadDetail,
  previewCodexThreadRebind, resumeCodexThread,
} from "../../../api/backend";
import type { Language } from "../../../i18n";
import type {
  CodexThreadDetailPage, CodexThreadRebindPreview, CodexThreadTimelineItem,
} from "../../../types";
import type { ThreadCopy } from "../copy";
import { formatSize, interpolate } from "../utils";
import { mergeThreadDetailPage } from "../detailPages";
import styles from "./index.module.less";

const PAGE_SIZE = 100;

interface ThreadDetailDrawerProps {
  sessionId: string | null;
  liveVersion?: string;
  close: () => void;
  language: Language;
  text: ThreadCopy;
  notify: (message: string) => void;
  reportError: (error: unknown) => void;
}

function formatTimestamp(value: string | null, language: Language) {
  if (!value) return "—";
  const parsed = new Date(value);
  if (Number.isNaN(parsed.getTime())) return value;
  return new Intl.DateTimeFormat(language === "zh" ? "zh-CN" : "en-US", {
    dateStyle: "medium",
    timeStyle: "medium",
  }).format(parsed);
}

function TimelineItem({ item, language, text }: {
  item: CodexThreadTimelineItem;
  language: Language;
  text: ThreadCopy;
}) {
  const isUser = item.kind === "message:user";
  const isAssistant = item.kind === "message:assistant";
  const label = isUser ? text.userMessage : isAssistant ? text.assistantMessage : text.toolCall;
  const Icon = isUser ? User : isAssistant ? Bot : Wrench;
  const status = item.toolStatus ? text[item.toolStatus] : null;
  return (
    <article className={`${styles.timelineItem} ${styles[item.kind.replace(":", "")] ?? ""}`}>
      <header>
        <Icon size={16} />
        <strong>{label}{item.toolName ? ` · ${item.toolName}` : ""}</strong>
        {status && <Tag color={item.toolStatus === "errored" ? "red" : undefined}>{status}</Tag>}
        {item.truncated && <Tag color="gold">{text.truncated}</Tag>}
        <time>{formatTimestamp(item.timestamp, language)}</time>
      </header>
      {item.text && <p>{item.text}</p>}
      {item.kind === "tool_call" && item.toolInput && (
        <details>
          <summary>{text.toolInput}</summary>
          <pre>{item.toolInput}</pre>
        </details>
      )}
      {item.kind === "tool_call" && item.toolOutput && (
        <details>
          <summary>{text.toolOutput}</summary>
          <pre>{item.toolOutput}</pre>
        </details>
      )}
    </article>
  );
}

export function ThreadDetailDrawer(props: ThreadDetailDrawerProps) {
  const { sessionId, liveVersion, close, language, text, notify, reportError } = props;
  const [detail, setDetail] = useState<CodexThreadDetailPage | null>(null);
  const [loading, setLoading] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  const [resuming, setResuming] = useState(false);
  const [rebindBusy, setRebindBusy] = useState(false);
  const [rebindPreview, setRebindPreview] = useState<CodexThreadRebindPreview | null>(null);
  const [loadedVersion, setLoadedVersion] = useState<string>();
  const [newerAvailable, setNewerAvailable] = useState(false);
  const [readError, setReadError] = useState<string | null>(null);
  const currentId = useRef(sessionId);
  const versionRef = useRef(liveVersion);
  const generation = useRef(0);
  const bodyRef = useRef<HTMLDivElement>(null);
  const pendingScroll = useRef<{ height: number; top: number } | "top" | null>(null);
  currentId.current = sessionId;
  versionRef.current = liveVersion;

  const loadLatest = useCallback(async (id: string, version?: string) => {
    const request = ++generation.current;
    setLoading(true);
    setLoadingMore(false);
    setReadError(null);
    try {
      const result = await loadCodexThreadDetail(id, 0, PAGE_SIZE, null, true);
      if (request !== generation.current || currentId.current !== id) return;
      pendingScroll.current = "top";
      setDetail(result);
      setLoadedVersion(version);
      setNewerAvailable(false);
    } catch (error) {
      if (request === generation.current && currentId.current === id) {
        setReadError(error instanceof Error ? error.message : String(error));
      }
    } finally {
      if (request === generation.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (!sessionId) {
      setDetail(null);
      setRebindPreview(null);
      return undefined;
    }
    setDetail(null);
    setRebindPreview(null);
    void loadLatest(sessionId, versionRef.current);
    return () => { generation.current += 1; };
  }, [loadLatest, sessionId]);

  useLayoutEffect(() => {
    const scroller = bodyRef.current?.closest<HTMLElement>(".ant-drawer-body");
    const previous = pendingScroll.current;
    pendingScroll.current = null;
    if (!scroller || previous == null) return;
    scroller.scrollTop = previous === "top" ? 0 : previous.top + scroller.scrollHeight - previous.height;
  }, [detail]);

  const loadMore = async (earlier = false) => {
    const offset = earlier ? detail?.previousOffset : detail?.nextOffset;
    if (!sessionId || !detail || detail.summary.sessionId !== sessionId || offset == null || loadingMore || loading) return;
    const request = generation.current;
    setLoadingMore(true);
    try {
      const page = await loadCodexThreadDetail(
        sessionId,
        offset,
        PAGE_SIZE,
        detail.revision,
      );
      if (request !== generation.current || currentId.current !== sessionId) return;
      if (earlier) {
        const scroller = bodyRef.current?.closest<HTMLElement>(".ant-drawer-body");
        if (scroller) pendingScroll.current = { height: scroller.scrollHeight, top: scroller.scrollTop };
      }
      setDetail((current) => current ? mergeThreadDetailPage(current, page, earlier) : current);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      if (request !== generation.current || currentId.current !== sessionId) return;
      if (!message.includes("session_revision_changed")) {
        reportError(error);
        return;
      }
      setNewerAvailable(true);
      notify(language === "zh" ? "会话已有新内容，当前阅读位置已保留；点击“查看最新”更新。" : "New content is available. Your reading position is retained; choose View latest to update.");
    } finally {
      if (request === generation.current) setLoadingMore(false);
    }
  };

  const copyResumeCommand = async () => {
    if (!detail?.summary.resumeCommand) return;
    try {
      await navigator.clipboard.writeText(detail.summary.resumeCommand);
      notify(text.commandCopied);
    } catch (error) {
      reportError(error);
    }
  };

  const resume = async (pickDirectory: boolean) => {
    if (!sessionId) return;
    setResuming(true);
    try {
      const targetCwd = pickDirectory ? await chooseCodexThreadDirectory() : null;
      if (pickDirectory && !targetCwd) return;
      const result = await resumeCodexThread(sessionId, targetCwd);
      notify(result.message);
    } catch (error) {
      reportError(error);
    } finally {
      setResuming(false);
    }
  };

  const prepareRebind = async () => {
    if (!sessionId) return;
    setRebindBusy(true);
    try {
      const targetCwd = await chooseCodexThreadDirectory(text.chooseRebindDirectoryTitle);
      if (!targetCwd) return;
      if (currentId.current !== sessionId) return;
      const preview = await previewCodexThreadRebind(sessionId, targetCwd);
      if (currentId.current === sessionId) setRebindPreview(preview);
    } catch (error) {
      reportError(error);
    } finally {
      setRebindBusy(false);
    }
  };

  const confirmRebind = async () => {
    if (!sessionId || !rebindPreview) return;
    setRebindBusy(true);
    try {
      const result = await confirmCodexThreadRebind(rebindPreview.confirmToken);
      if (currentId.current === sessionId) {
        setRebindPreview(null);
        await loadLatest(sessionId, versionRef.current);
      }
      notify(result.message);
    } catch (error) {
      reportError(error);
    } finally {
      setRebindBusy(false);
    }
  };

  const visibleDetail = detail?.summary.sessionId === sessionId ? detail : null;
  const summary = visibleDetail?.summary;
  const hasUpdates = newerAvailable || Boolean(summary && liveVersion && loadedVersion !== liveVersion);
  return (
    <Drawer
      className={styles.detailDrawer}
      open={sessionId !== null}
      onClose={close}
      width="min(920px, 94vw)"
      title={summary?.title || text.detailTitle}
    >
      {readError && <Alert type="warning" message={readError} showIcon />}
      {loading && !summary && <div className={styles.detailLoading}><Spin /><span>{text.detailLoading}</span></div>}
      {summary && visibleDetail && (
        <div className={styles.detailBody} ref={bodyRef}>
          <div className={styles.readingStatus}>
            <span>{hasUpdates
              ? (language === "zh" ? "会话有更新 · 保留当前阅读位置" : "Updates available · Reading position retained")
              : (language === "zh" ? "最新消息优先 · 向前加载更早记录" : "Latest messages first · Load earlier history above")}</span>
            <Button size="small" loading={loading} onClick={() => sessionId && void loadLatest(sessionId, versionRef.current)}>
              {language === "zh" ? "查看最新" : "View latest"}
            </Button>
          </div>
          {(summary.skippedRecordCount ?? 0) > 0 && <Alert type="warning" showIcon message={language === "zh"
            ? `${summary.skippedRecordCount} 条过大或不完整的记录未展示，其余记录可正常阅读；原始历史未修改。`
            : `${summary.skippedRecordCount} oversized or incomplete records are not displayed. Other records remain readable; original history is unchanged.`} />}
          <section className={styles.summaryPanel}>
            <div className={styles.summaryIdentity}>
              <strong>{summary.sessionId}</strong>
              <span>{summary.cwd}</span>
            </div>
            <div className={styles.summaryMeta}>
              <span>{text.startedAt}: {formatTimestamp(summary.startedAt, language)}</span>
              <span>{text.updatedAt}: {formatTimestamp(summary.updatedAt, language)}</span>
              <span>{summary.lineCount.toLocaleString()} {text.lineCount}</span>
              <span>{summary.eventCount.toLocaleString()} {text.eventCount}</span>
              <span>{summary.toolCallCount.toLocaleString()} {text.toolCallCount}</span>
              <span>{formatSize(summary.sizeBytes)}</span>
              {(summary.segmentCount ?? 1) > 1 && <span>{language === "zh" ? `${summary.segmentCount} 个历史分段` : `${summary.segmentCount} history segments`}</span>}
              <span>{text.source}: {summary.source}</span>
              <span>{text.modelProvider}: {summary.modelProvider}</span>
              <span>{text.cliVersion}: {summary.cliVersion}</span>
            </div>
            <div className={styles.excerptGrid}>
              <article><strong>{text.userSummary}</strong><p>{summary.userPromptExcerpt || "—"}</p></article>
              <article><strong>{text.assistantSummary}</strong><p>{summary.latestAgentMessageExcerpt || "—"}</p></article>
            </div>
            {summary.resumeCommand && (
              <div className={styles.resumePanel}>
                <div className={styles.resumeCommand}>
                  <Terminal size={16} />
                  <code>{summary.resumeCommand}</code>
                  <Button size="small" icon={<Copy size={14} />} onClick={() => void copyResumeCommand()}>
                    {text.copyCommand}
                  </Button>
                </div>
                <div className={styles.resumeActions}>
                  <Button type="primary" loading={resuming} icon={<Play size={15} />}
                    onClick={() => void resume(false)}>
                    {text.resumeNow}
                  </Button>
                  <Button disabled={resuming} icon={<FolderOpen size={15} />}
                    onClick={() => void resume(true)}>
                    {text.resumeElsewhere}
                  </Button>
                  <Button disabled={resuming} loading={rebindBusy} icon={<FolderCog size={15} />}
                    onClick={() => void prepareRebind()}>
                    {text.rebindDirectory}
                  </Button>
                  <span>{text.resumeElsewhereHint}</span>
                </div>
              </div>
            )}
          </section>
          <section className={styles.timeline}>
            {visibleDetail.previousOffset != null && <Button loading={loadingMore} disabled={loading}
              onClick={() => void loadMore(true)}>{language === "zh" ? "加载更早记录" : "Load earlier history"}</Button>}
            {visibleDetail.items.length ? visibleDetail.items.map((item) => (
              <TimelineItem key={item.id} item={item} language={language} text={text} />
            )) : <div className={styles.detailEmpty}>{text.detailEmpty}</div>}
            <footer>
              <span>{interpolate(text.timelineProgress, {
                shown: visibleDetail.items.length,
                total: visibleDetail.total,
              })}</span>
              {visibleDetail.nextOffset != null && (
                <Button loading={loadingMore} onClick={() => void loadMore()}>{text.loadMore}</Button>
              )}
            </footer>
          </section>
        </div>
      )}
      <Modal
        className={styles.rebindModal}
        open={rebindPreview !== null}
        title={text.rebindTitle}
        okText={text.confirmRebind}
        cancelText={text.close}
        confirmLoading={rebindBusy}
        closable={!rebindBusy}
        maskClosable={!rebindBusy}
        keyboard={!rebindBusy}
        onCancel={() => setRebindPreview(null)}
        onOk={() => void confirmRebind()}
      >
        {rebindPreview && (
          <div className={styles.rebindPreview}>
            <p>{text.rebindDescription}</p>
            <dl>
              <div><dt>{text.rebindOld}</dt><dd>{rebindPreview.oldCwd}</dd></div>
              <div><dt>{text.rebindNew}</dt><dd>{rebindPreview.newCwd}</dd></div>
            </dl>
            <p>{interpolate(text.rebindImpact, {
              files: rebindPreview.rolloutFileCount,
              db: rebindPreview.stateDatabaseRowCount,
              catalog: rebindPreview.catalogRowCount,
            })}</p>
            <ul>
              <li>{text.rebindIndex}</li>
              <li>{text.rebindRollback}</li>
            </ul>
          </div>
        )}
      </Modal>
    </Drawer>
  );
}
