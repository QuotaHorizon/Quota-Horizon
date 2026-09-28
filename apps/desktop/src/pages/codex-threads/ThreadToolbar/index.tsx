import { Button, Input, Select } from "antd";
import { Archive, Search, Trash2, X } from "lucide-react";
import type { Dispatch, SetStateAction } from "react";
import type { CodexThreadKind, CodexThreadStatus } from "../../../types";
import type { ThreadCopy } from "../copy";
import styles from "./index.module.less";

interface ThreadToolbarProps {
  text: ThreadCopy;
  query: string;
  setQuery: Dispatch<SetStateAction<string>>;
  queueSearch: (query: string) => void;
  search: () => void;
  clearSearch: () => void;
  kind: CodexThreadKind | "all";
  setKind: Dispatch<SetStateAction<CodexThreadKind | "all">>;
  status: CodexThreadStatus | "all";
  setStatus: Dispatch<SetStateAction<CodexThreadStatus | "all">>;
  selectedCount: number;
  canArchiveSelected: boolean;
  busy: boolean;
  confirmArchive: () => void;
  confirmTrash: () => void;
}

export function ThreadToolbar(props: ThreadToolbarProps) {
  const { text, query, setQuery, queueSearch, search, clearSearch, kind, setKind } = props;
  const {
    status, setStatus, selectedCount, canArchiveSelected, busy, confirmArchive, confirmTrash,
  } = props;
  return (
    <div className={styles.codexThreadToolbar}>
      <Input
        maxLength={256}
        aria-label={text.searchPlaceholder}
        value={query}
        onChange={(event) => {
          const nextQuery = event.target.value;
          setQuery(nextQuery);
          queueSearch(nextQuery);
        }}
        onPressEnter={search}
        prefix={<Search size={17} />}
        placeholder={text.searchPlaceholder}
        suffix={query ? (
          <button className={styles.threadInputClear} onClick={clearSearch} aria-label={text.clear}>
            <X size={15} />
          </button>
        ) : null}
      />
      <Select value={status} onChange={setStatus} options={[
        { value: "active", label: text.active },
        { value: "archived", label: text.archived },
        { value: "all", label: text.allStatuses },
      ]} />
      <Select value={kind} onChange={setKind} options={[
        { value: "conversation", label: text.conversation },
        { value: "external", label: text.external },
        { value: "subagent", label: text.subagent },
        { value: "all", label: text.allKinds },
      ]} />
      <Button
        icon={<Archive size={16} />}
        disabled={!canArchiveSelected || busy || status === "archived"}
        onClick={confirmArchive}
      >
        {text.archive} ({selectedCount})
      </Button>
      <Button
        icon={<Trash2 size={16} />}
        danger
        disabled={!selectedCount || busy}
        onClick={confirmTrash}
      >
        {text.moveToBin} ({selectedCount})
      </Button>
    </div>
  );
}
