import { useCallback } from "react";
import type { Dispatch, SetStateAction } from "react";
import { Modal } from "antd";
import {
  confirmCodexThreadArchive,
  previewCodexThreadArchive,
} from "../../api/backend";
import type { CodexThreadArchivePreview } from "../../types";
import type { ThreadCopy } from "./copy";
import { formatSize, interpolate } from "./utils";

interface ArchiveOptions {
  selected: Set<string>;
  clearSelection: () => void;
  text: ThreadCopy;
  notify: (message: string) => void;
  reportError: (error: unknown) => void;
  refresh: () => Promise<void>;
  setBusy: Dispatch<SetStateAction<boolean>>;
}

export function useArchive(options: ArchiveOptions) {
  const { selected, clearSelection, text, notify, reportError, refresh, setBusy } = options;

  return useCallback(async (sessionIds = [...selected]) => {
    if (!sessionIds.length) {
      notify(text.pickOne);
      return;
    }
    setBusy(true);
    let preview: CodexThreadArchivePreview;
    try {
      preview = await previewCodexThreadArchive(sessionIds);
    } catch (error) {
      reportError(error);
      return;
    } finally {
      setBusy(false);
    }
    const visibleItems = preview.items.slice(0, 5);
    const hiddenCount = Math.max(0, preview.items.length - visibleItems.length);
    Modal.confirm({
      title: text.archive,
      width: 560,
      content: (
        <div style={{ maxWidth: 480, lineHeight: 1.55 }}>
          <strong>{interpolate(text.archivePreviewSummary, {
            count: preview.affectedCount,
            size: formatSize(preview.totalSizeBytes),
          })}</strong>
          <ul>
            <li>{text.archiveConflictCheck}</li>
            <li>{text.archiveIndexPreserved}</li>
            <li>{text.archiveRollback}</li>
          </ul>
          {visibleItems.map((item) => <div key={item.sessionId}>{item.title || item.sessionId}</div>)}
          {hiddenCount > 0 && <small>{interpolate(text.trashMoreItems, { count: hiddenCount })}</small>}
        </div>
      ),
      okText: text.archive,
      cancelText: text.close,
      onOk: async () => {
        setBusy(true);
        try {
          const result = await confirmCodexThreadArchive(preview.confirmToken);
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
  }, [clearSelection, notify, refresh, reportError, selected, setBusy, text]);
}
