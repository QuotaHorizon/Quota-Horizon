import { useCallback, useEffect, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { getPublicResetArchive, isDesktopApp } from "../../api/backend";
import type { Language } from "../../i18n";
import { PublicResetArchiveView } from "./ArchiveView";
import { LiveSourcesPanel } from "./LiveSourcesPanel";
import type { PublicResetArchive } from "./types";
import styles from "./index.module.less";

export function ResetIntelligencePage({ language, notify }: { language: Language; notify: (message: string) => void }) {
  const [archive, setArchive] = useState<PublicResetArchive | null>(null);
  const [failed, setFailed] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const zh = language === "zh";
  useEffect(() => {
    if (!isDesktopApp) return;
    let active = true;
    setFailed(false);
    void getPublicResetArchive().then((next) => { if (active) setArchive(next); })
      .catch(() => { if (active) setFailed(true); });
    return () => { active = false; };
  }, [attempt]);
  const onOpenSource = useCallback((url: string) => {
    void openUrl(url).catch(() => notify(language === "zh" ? "未能打开来源链接，请稍后重试。" : "The source link could not be opened. Please try again."));
  }, [language, notify]);

  return <div className={styles.page}>
    {!isDesktopApp && <p className={styles.empty}>{zh ? "请在 Horizon 桌面应用中查看本地资料与账号窗口。" : "Open the Horizon desktop app to view the local archive and account windows."}</p>}
    {isDesktopApp && <LiveSourcesPanel archive={archive} language={language} onOpenSource={onOpenSource} archiveContent={<>
    {failed && <div className={styles.empty} role="alert">
      <p>{zh ? "附带资料读取失败，请重试。" : "The bundled archive could not be loaded. Try again."}</p>
      <button onClick={() => setAttempt((value) => value + 1)}>{zh ? "重新载入资料" : "Reload archive"}</button>
    </div>}
    {!failed && !archive && <p className={styles.empty} role="status">{zh ? "正在读取应用附带资料…" : "Loading the bundled archive…"}</p>}
    {archive && <details className={styles.sourceDisclosure}>
      <summary>{zh ? "资料归档" : "Source archive"}</summary>
      <PublicResetArchiveView archive={archive} language={language} onOpenSource={onOpenSource} />
    </details>}
    </>} />}
  </div>;
}
