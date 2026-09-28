import { useState } from "react";
import type { Dispatch, SetStateAction } from "react";
import { Modal } from "antd";
import {
  confirmCodexThreadPurge,
  confirmCodexThreadRestore,
  confirmCodexThreadsToBin,
  loadCodexThreadBin,
  previewCodexThreadPurge,
  previewCodexThreadsToBin,
  previewCodexThreadRestore,
} from "../../api/backend";
import type {
  CodexThreadBinEntry,
  CodexThreadPurgePreview,
  CodexThreadRestorePreview,
  CodexThreadTrashPreview,
} from "../../types";
import type { ThreadCopy } from "./copy";
import { formatSize, interpolate } from "./utils";

interface TrashOptions {
  selected: Set<string>;
  clearSelection: () => void;
  text: ThreadCopy;
  notify: (message: string) => void;
  reportError: (error: unknown) => void;
  refresh: () => Promise<void>;
  setBusy: Dispatch<SetStateAction<boolean>>;
}

export function useTrash(options: TrashOptions) {
  const { selected, clearSelection, text, notify, reportError, refresh, setBusy } = options;
  const [open, setOpen] = useState(false);
  const [entries, setEntries] = useState<CodexThreadBinEntry[]>([]);
  const [binSelected, setBinSelected] = useState<Set<string>>(new Set());

  const openBin = async () => {
    setBusy(true);
    try {
      setEntries(await loadCodexThreadBin());
      setBinSelected(new Set());
      setOpen(true);
    } catch (error) {
      reportError(error);
    } finally {
      setBusy(false);
    }
  };
  const reload = async () => {
    setEntries(await loadCodexThreadBin());
    setBinSelected(new Set());
    await refresh();
  };
  const restore = async () => {
    if (!binSelected.size) {
      notify(text.pickOne);
      return;
    }
    const sessionIds = [...binSelected];
    setBusy(true);
    let preview: CodexThreadRestorePreview;
    try {
      preview = await previewCodexThreadRestore(sessionIds);
    } catch (error) {
      reportError(error);
      return;
    } finally {
      setBusy(false);
    }
    const visibleItems = preview.items.slice(0, 5);
    const hiddenCount = Math.max(0, preview.items.length - visibleItems.length);
    Modal.confirm({
      title: text.restore,
      width: 560,
      content: (
        <div style={{ maxWidth: 480, lineHeight: 1.55 }}>
          <strong>{interpolate(text.restorePreviewSummary, {
            count: preview.affectedCount,
            size: formatSize(preview.totalSizeBytes),
          })}</strong>
          <ul>
            <li>{text.restoreConflictCheck}</li>
            <li>{text.restoreRollback}</li>
          </ul>
          {visibleItems.map((item) => <div key={item.sessionId}>{item.title || item.sessionId}</div>)}
          {hiddenCount > 0 && <small>{interpolate(text.trashMoreItems, { count: hiddenCount })}</small>}
        </div>
      ),
      okText: text.restore,
      cancelText: text.close,
      onOk: async () => {
        setBusy(true);
        try {
          const result = await confirmCodexThreadRestore(preview.confirmToken);
          notify(result.message);
          await reload();
        } catch (error) {
          reportError(error);
        } finally {
          setBusy(false);
        }
      },
    });
  };
  const confirmDelete = async (empty = false) => {
    if (!empty && !binSelected.size) {
      notify(text.pickOne);
      return;
    }
    setBusy(true);
    let preview: CodexThreadPurgePreview;
    try {
      preview = await previewCodexThreadPurge(empty ? [] : [...binSelected], empty);
    } catch (error) {
      reportError(error);
      return;
    } finally {
      setBusy(false);
    }
    const visibleItems = preview.items.slice(0, 5);
    const hiddenCount = Math.max(0, preview.items.length - visibleItems.length);
    Modal.confirm({
      title: empty ? text.emptyBin : text.deleteForever,
      width: 560,
      content: (
        <div style={{ maxWidth: 480, lineHeight: 1.55 }}>
          <p>{empty ? text.confirmEmpty : text.confirmDelete}</p>
          <strong>{interpolate(text.purgePreviewSummary, {
            count: preview.affectedCount,
            size: formatSize(preview.totalSizeBytes),
          })}</strong>
          <ul>
            <li>{text.purgeRestorePoint}</li>
            <li>{text.purgePermanent}</li>
          </ul>
          {visibleItems.map((item) => <div key={item.sessionId}>{item.title || item.sessionId}</div>)}
          {hiddenCount > 0 && <small>{interpolate(text.trashMoreItems, { count: hiddenCount })}</small>}
        </div>
      ),
      okText: empty ? text.emptyBin : text.deleteForever,
      cancelText: text.close,
      okButtonProps: { danger: true },
      onOk: async () => {
        setBusy(true);
        try {
          const result = await confirmCodexThreadPurge(preview.confirmToken);
          notify(result.message);
          await reload();
        } catch (error) {
          reportError(error);
        } finally {
          setBusy(false);
        }
      },
    });
  };
  const confirmMove = async () => {
    if (!selected.size) {
      notify(text.pickOne);
      return;
    }
    const sessionIds = [...selected];
    setBusy(true);
    let preview: CodexThreadTrashPreview;
    try {
      preview = await previewCodexThreadsToBin(sessionIds);
    } catch (error) {
      reportError(error);
      return;
    } finally {
      setBusy(false);
    }
    const visibleItems = preview.items.slice(0, 5);
    const hiddenCount = Math.max(0, preview.items.length - visibleItems.length);
    Modal.confirm({
      title: text.moveToBin,
      width: 560,
      content: (
        <div style={{ maxWidth: 480, lineHeight: 1.55 }}>
          <p>{text.confirmTrash}</p>
          <strong>{interpolate(text.trashPreviewSummary, {
            count: preview.affectedCount,
            size: formatSize(preview.totalSizeBytes),
          })}</strong>
          <ul>
            <li>{text.trashRestorePoint}</li>
            <li>{text.trashRollback}</li>
          </ul>
          {visibleItems.map((item) => <div key={item.sessionId}>{item.title || item.sessionId}</div>)}
          {hiddenCount > 0 && <small>{interpolate(text.trashMoreItems, { count: hiddenCount })}</small>}
        </div>
      ),
      okText: text.moveToBin,
      cancelText: text.close,
      okButtonProps: { danger: true },
      onOk: async () => {
        setBusy(true);
        try {
          const result = await confirmCodexThreadsToBin(preview.confirmToken);
          notify(result.message);
          clearSelection();
          await refresh();
        } catch (error) {
          reportError(error);
        } finally {
          setBusy(false);
        }
      },
    });
  };

  return {
    open, setOpen, entries, selected: binSelected, setSelected: setBinSelected,
    openBin, restore, confirmDelete, confirmMove,
  };
}
