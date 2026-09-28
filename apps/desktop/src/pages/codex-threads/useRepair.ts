import { useEffect, useState } from "react";
import {
  confirmCodexThreadVisibilityRepair,
  previewCodexThreadVisibilityRepair,
} from "../../api/backend";
import type { CodexThreadVisibilityRepairPreview } from "../../types";
import type { ThreadCopy } from "./copy";

interface RepairOptions {
  selected: Set<string>;
  text: ThreadCopy;
  notify: (message: string) => void;
  reportError: (error: unknown) => void;
  refresh: () => Promise<void>;
}

export function useRepair(options: RepairOptions) {
  const { selected, text, notify, reportError, refresh } = options;
  const [open, setOpen] = useState(false);
  const [mode, setMode] = useState<"quick" | "deep">("quick");
  const [scope, setScope] = useState<"all" | "selected">("all");
  const [preview, setPreview] = useState<CodexThreadVisibilityRepairPreview | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    setPreview(null);
  }, [selected]);

  const runPreview = async () => {
    if (scope === "selected" && !selected.size) {
      notify(text.selectedEmpty);
      return;
    }
    setBusy(true);
    try {
      const sessionIds = scope === "selected" ? [...selected] : null;
      setPreview(await previewCodexThreadVisibilityRepair({ mode, sessionIds }));
    } catch (error) {
      reportError(error);
    } finally {
      setBusy(false);
    }
  };

  const confirm = async () => {
    if (!preview) return;
    setBusy(true);
    try {
      const result = await confirmCodexThreadVisibilityRepair(preview.confirmToken);
      notify(result.message);
      setOpen(false);
      setPreview(null);
      await refresh();
    } catch (error) {
      setPreview(null);
      reportError(error);
    } finally {
      setBusy(false);
    }
  };
  const openModal = () => {
    setPreview(null);
    setOpen(true);
  };
  const updateOpen = (value: boolean) => {
    if (!value) setPreview(null);
    setOpen(value);
  };
  const updateMode = (value: "quick" | "deep") => {
    setMode(value);
    setPreview(null);
  };
  const updateScope = (value: "all" | "selected") => {
    setScope(value);
    setPreview(null);
  };

  return {
    open,
    setOpen: updateOpen,
    mode,
    updateMode,
    scope,
    updateScope,
    preview,
    busy,
    runPreview,
    confirm,
    openModal,
  };
}
