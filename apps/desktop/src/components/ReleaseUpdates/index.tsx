import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { Language } from "../../i18n";
import styles from "./index.module.less";

export interface ReleaseUpdate {
  currentVersion: string;
  latestVersion: string | null;
  available: boolean;
  state: "available" | "up_to_date" | "platform_pending" | "no_release" | "unsupported_platform";
  notes: string;
  url: string;
}
const PREFERENCE_KEY = "quotahorizon.release-notice.v1";
const SIX_HOURS = 6 * 3600_000;
interface Preference { ignored?: string; laterUntil?: number; }
function preferences(): Preference {
  try {
    const value = JSON.parse(localStorage.getItem(PREFERENCE_KEY) ?? "{}");
    return value && typeof value === "object" ? value : {};
  } catch { return {}; }
}
export function shouldNotify(update: ReleaseUpdate, preference: Preference, now: number) {
  return update.available && update.state === "available" && update.latestVersion !== preference.ignored && now >= (preference.laterUntil ?? 0);
}

export function releaseCheckMessage(update: ReleaseUpdate, language: Language) {
  const zh = language === "zh";
  switch (update.state) {
    case "up_to_date": return zh ? "当前已是最新可用版本" : "You have the latest available version";
    case "platform_pending": return zh ? `v${update.latestVersion} 已发布，本平台安装包尚未就绪` : `v${update.latestVersion} is published; the installer for this platform is not ready yet`;
    case "no_release": return zh ? "暂未找到正式发布版本" : "No stable release is currently published";
    case "unsupported_platform": return zh ? "当前平台暂无公开安装包" : "No public installer is available for this platform";
    case "available": return zh ? `发现新版本 v${update.latestVersion}` : `Update available: v${update.latestVersion}`;
    default: return zh ? "未能确认发布状态，请稍后重试" : "Unable to confirm release status. Please try again later";
  }
}

export function ReleaseUpdateDialog({ update, language, onLater, onIgnore, onDownload }: {
  update: ReleaseUpdate; language: Language; onLater: () => void; onIgnore: () => void; onDownload: () => void;
}) {
  const zh = language === "zh";
  useEffect(() => {
    const previous = document.activeElement;
    return () => { if (previous instanceof HTMLElement && previous.isConnected) previous.focus(); };
  }, []);
  return <div className={styles.backdrop} onClick={onLater}>
    <section className={styles.dialog} role="dialog" aria-modal="true" aria-labelledby="release-update-title" onClick={event => event.stopPropagation()} onKeyDown={event => {
      if (event.key === "Escape") onLater();
      if (event.key === "Tab") {
        const buttons = event.currentTarget.querySelectorAll("button");
        const first = buttons[0], last = buttons[buttons.length - 1];
        if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); }
        else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
      }
    }}>
      <h2 id="release-update-title">{zh ? "发现新版本" : "Update available"} · v{update.latestVersion}</h2>
      <p>{zh ? `当前版本 v${update.currentVersion}` : `Current version v${update.currentVersion}`}</p>
      {update.notes && <pre>{update.notes}</pre>}
      {!update.notes && <p>{zh ? "更新说明和安装包可在下载页面查看。" : "Release notes and installers are available on the download page."}</p>}
      <footer><button onClick={onIgnore}>{zh ? "忽略此版本" : "Skip this version"}</button><button onClick={onLater}>{zh ? "稍后" : "Later"}</button><button className={styles.primary} onClick={onDownload} autoFocus>{zh ? "查看并下载" : "View and download"}</button></footer>
    </section>
  </div>;
}

export function ReleaseUpdates({ language, manualRequest, notify, onCheckingChange }: { language: Language; manualRequest: number; notify: (text: string) => void; onCheckingChange?: (checking: boolean) => void }) {
  const [update, setUpdate] = useState<ReleaseUpdate | null>(null);
  const busy = useRef(false), manualPending = useRef(false), alive = useRef(true), context = useRef({ language, notify, onCheckingChange });
  context.current = { language, notify, onCheckingChange };
  const check = useCallback(async (manual: boolean) => {
    if (!("__TAURI_INTERNALS__" in window)) return;
    // A manual request joins an ongoing automatic check instead of disappearing.
    if (manual) manualPending.current = true;
    if (busy.current) return;
    busy.current = true;
    context.current.onCheckingChange?.(true);
    try {
      const result = await invoke<ReleaseUpdate>("check_release_update");
      if (!alive.current) return;
      if (result.available && result.state === "available" && (manualPending.current || shouldNotify(result, preferences(), Date.now()))) setUpdate(result);
      else if (manualPending.current) context.current.notify(releaseCheckMessage(result, context.current.language));
    } catch {
      if (manualPending.current && alive.current) context.current.notify(context.current.language === "zh" ? "暂时无法检查更新，请稍后重试" : "Unable to check for updates. Please try again later.");
    } finally { busy.current = false; manualPending.current = false; if (alive.current) context.current.onCheckingChange?.(false); }
  }, []);
  useEffect(() => {
    alive.current = true;
    const first = window.setTimeout(() => void check(false), 15_000);
    const interval = window.setInterval(() => void check(false), SIX_HOURS);
    return () => { alive.current = false; clearTimeout(first); clearInterval(interval); };
  }, [check]);
  useEffect(() => { if (manualRequest > 0) void check(true); }, [manualRequest, check]);
  const dismiss = (ignore: boolean) => {
    if (!update) return;
    const preference = { ...preferences(), ...(ignore ? { ignored: update.latestVersion ?? undefined } : { laterUntil: Date.now() + 24 * 3600_000 }) };
    try { localStorage.setItem(PREFERENCE_KEY, JSON.stringify(preference)); } catch { /* A closed notice still closes when preferences are unavailable. */ }
    setUpdate(null);
  };
  return update && <ReleaseUpdateDialog update={update} language={language} onLater={() => dismiss(false)} onIgnore={() => dismiss(true)} onDownload={() => {
    void openUrl(update.url).then(() => dismiss(false)).catch(() => notify(language === "zh" ? "无法打开下载页面，请稍后重试" : "Unable to open the download page. Please try again."));
  }} />;
}
