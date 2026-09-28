import { useCallback, useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { Modal } from "antd";
import { restartChatGpt } from "../../api/backend";
import type { Language } from "../../i18n";
import { threadCopy } from "../codex-threads/copy";
import { ThreadList } from "../codex-threads/ThreadList";
import { ThreadDetailDrawer } from "../codex-threads/ThreadDetail";
import { RepairModal, TransferModal, TrashModal } from "../codex-threads/ThreadModals";
import { ThreadToolbar } from "../codex-threads/ThreadToolbar";
import { ThreadTopbar } from "../codex-threads/ThreadTopbar";
import { useRepair } from "../codex-threads/useRepair";
import { useArchive } from "../codex-threads/useArchive";
import { useMigration } from "../codex-threads/useMigration";
import { useThreadList } from "../codex-threads/useThreadList";
import { SearchCoverage } from "../codex-threads/SearchCoverage";
import { useTransfer } from "../codex-threads/useTransfer";
import { useTrash } from "../codex-threads/useTrash";
import styles from "./index.module.less";

interface CodexThreadsPageProps {
  language: Language;
  notify: (message: string) => void;
}

export function CodexThreadsPage({ language, notify }: CodexThreadsPageProps) {
  const text = threadCopy[language];
  const [busy, setBusy] = useState(false);
  const [topbarHost, setTopbarHost] = useState<HTMLElement | null>(null);
  const [detailSessionId, setDetailSessionId] = useState<string | null>(null);
  const reportError = useCallback(
    (error: unknown) => notify(error instanceof Error ? error.message : String(error)),
    [notify],
  );
  const list = useThreadList(reportError);
  const refresh = async () => {
    if (!await list.refresh(list.appliedQuery, true)) {
      throw new Error(language === "zh" ? "会话列表更新失败，已保留上次结果。" : "Session update failed; the previous result is retained.");
    }
  };
  const transfer = useTransfer({
    selected: list.selected, text, notify, reportError, refresh, setBusy,
  });
  const trash = useTrash({
    selected: list.selected,
    clearSelection: () => list.setSelected(new Set()),
    text,
    notify,
    reportError,
    refresh,
    setBusy,
  });
  const repair = useRepair({ selected: list.selected, text, notify, reportError, refresh });
  const migrate = useMigration({
    text,
    notify,
    reportError,
    refresh,
    setBusy,
    clearSelection: () => list.setSelected(new Set()),
  });
  const archive = useArchive({
    selected: list.selected,
    clearSelection: () => list.setSelected(new Set()),
    text,
    notify,
    reportError,
    refresh,
    setBusy,
  });

  useEffect(() => {
    setTopbarHost(document.getElementById("codex-thread-topbar-actions"));
  }, []);

  const runSync = async () => {
    setBusy(true);
    try {
      await refresh();
      notify(text.syncSuccess);
    } catch (error) {
      reportError(error);
    } finally {
      setBusy(false);
    }
  };

  const runRestartChatGpt = async () => {
    setBusy(true);
    try {
      await restartChatGpt();
      notify(text.restartChatGptSuccess);
    } catch (error) {
      reportError(error);
    } finally {
      setBusy(false);
    }
  };

  const confirmRestartChatGpt = () => {
    Modal.confirm({
      title: text.restartChatGptConfirmTitle,
      content: <span className="compact-confirm-copy">{text.restartChatGptConfirmDescription}</span>,
      okText: text.restartChatGpt,
      cancelText: text.close,
      okButtonProps: { danger: true },
      onOk: runRestartChatGpt,
    });
  };

  const confirmMigration = (sessionIds: string[]) => {
    Modal.confirm({
      title: text.migrateConfirmTitle,
      content: <span className="compact-confirm-copy">{text.migrateConfirmDescription}</span>,
      okText: text.migrate,
      cancelText: text.close,
      onOk: () => migrate(sessionIds),
    });
  };

  const detailThread = list.visibleThreads.find((item) => item.sessionId === detailSessionId);

  return (
    <>
      {topbarHost && createPortal(
        <ThreadTopbar
          text={text}
          busy={busy}
          selectedCount={list.selected.size}
          runSync={() => void runSync()}
          restartChatGpt={confirmRestartChatGpt}
          openImport={() => void transfer.openImport()}
          openExport={() => void transfer.openExport()}
          migrateSelected={() => confirmMigration([...list.selected])}
          openRepair={repair.openModal}
          openBin={() => void trash.openBin()}
        />,
        topbarHost,
      )}
      <div className={styles.codexThreadManager}>
        <div className={styles.liveStatus} role="status">
          <span>{list.updateFailed
            ? (language === "zh" ? "更新暂不可用 · 已保留上次列表，将自动重试" : "Update unavailable · Previous list retained; retrying automatically")
            : (language === "zh" ? "自动更新 · 每 5 秒检查本地变化" : "Auto-updating · Checks local changes every 5 seconds")}</span>
          <time>{list.updatedAt ? `${language === "zh" ? "最近读取" : "Last read"} ${new Intl.DateTimeFormat(language === "zh" ? "zh-CN" : "en-US", {
            hour: "2-digit", minute: "2-digit", second: "2-digit", hourCycle: "h23",
          }).format(list.updatedAt)}` : (language === "zh" ? "正在读取…" : "Reading…")}</time>
        </div>
        <ThreadToolbar
          text={text}
          query={list.query}
          setQuery={list.setQuery}
          queueSearch={list.queueSearch}
          search={list.search}
          clearSearch={list.clearSearch}
          kind={list.kind}
          setKind={list.setKind}
          status={list.status}
          setStatus={list.setStatus}
          selectedCount={list.selected.size}
          canArchiveSelected={list.canArchiveSelected}
          busy={busy}
          confirmArchive={() => void archive()}
          confirmTrash={trash.confirmMove}
        />
        <SearchCoverage coverage={list.searchCoverage} matches={list.visibleThreads.length} language={language}
          searching={list.loading && !!list.query.trim()} stale={list.searchStale} stopped={list.searchStopped}
          onStop={list.stopSearch} onRestart={list.search} />
        <ThreadList
          language={language}
          text={text}
          loading={list.loading}
          searchIncomplete={list.searchStopped || (list.searchCoverage?.incompleteSessions ?? 0) > 0 || (list.searchCoverage?.pendingSessions ?? 0) > 0}
          appliedQuery={list.appliedQuery}
          groups={list.groups}
          selected={list.selected}
          setSelected={list.setSelected}
          expanded={list.expanded}
          tokens={list.tokens}
          visibleCount={list.visibleThreads.length}
          allVisibleSelected={list.allVisibleSelected}
          someVisibleSelected={list.someVisibleSelected}
          toggleAll={list.toggleAll}
          toggleThread={list.toggleThread}
          toggleGroup={list.toggleGroup}
          notify={notify}
          reportError={reportError}
          migrate={(id) => confirmMigration([id])}
          archive={(id) => void archive([id])}
          openDetail={setDetailSessionId}
        />
        <ThreadDetailDrawer
          sessionId={detailSessionId}
          liveVersion={detailThread ? `${detailThread.updatedAt}:${detailThread.sizeBytes}` : undefined}
          close={() => setDetailSessionId(null)}
          language={language}
          text={text}
          notify={notify}
          reportError={reportError}
        />
        <TrashModal
          open={trash.open}
          setOpen={trash.setOpen}
          entries={trash.entries}
          selected={trash.selected}
          setSelected={trash.setSelected}
          busy={busy}
          text={text}
          language={language}
          restore={() => void trash.restore()}
          confirmDelete={trash.confirmDelete}
        />
        <TransferModal
          open={transfer.open}
          setOpen={transfer.setOpen}
          mode={transfer.mode}
          preview={transfer.preview}
          selected={transfer.selected}
          setSelected={transfer.setSelected}
          busy={busy}
          text={text}
          commit={() => void transfer.commit()}
        />
        <RepairModal
          open={repair.open}
          setOpen={repair.setOpen}
          mode={repair.mode}
          updateMode={repair.updateMode}
          scope={repair.scope}
          updateScope={repair.updateScope}
          preview={repair.preview}
          busy={repair.busy}
          selectedCount={list.selected.size}
          text={text}
          runPreview={() => void repair.runPreview()}
          confirm={() => void repair.confirm()}
        />
      </div>
    </>
  );
}
