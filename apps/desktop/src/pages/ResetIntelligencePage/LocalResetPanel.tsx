import { useEffect, useState, type ReactNode } from "react";
import { CalendarClock } from "lucide-react";
import type { DesktopStatusEnvelope } from "../../../../capacity-preview/src/status";
import { getCapacityCachedStatus, getCapacityStatus, getCapacityStatusSnapshot, isDesktopApp, subscribeToCapacityStatus } from "../../api/backend";
import { acceptNewerEnvelope } from "../../components/CapacityPopover/presentation";
import { percentValueLabel, resetCountdown } from "../../components/CapacityPopover/presentation";
import type { Language } from "../../i18n";
import { localResetFacts, publicEvidenceTime } from "./presentation";
import { LocalQuotaChanges } from "./LocalQuotaChanges";
import { resetCreditPresentation } from "../CapacityPage/resetCreditPresentation";
import styles from "./index.module.less";

export function LocalResetFacts({ envelope, language, now, failed = false, observations }: {
  envelope: DesktopStatusEnvelope | null;
  language: Language;
  now: number;
  failed?: boolean;
  observations?: ReactNode;
}) {
  const zh = language === "zh";
  const facts = localResetFacts(envelope);
  const credits = resetCreditPresentation(envelope, language);
  return <section className={`${styles.section} ${styles.localPanel}`} aria-labelledby="local-reset-facts">
    <h2 id="local-reset-facts"><CalendarClock size={19} aria-hidden="true" />{zh ? "当前账号" : "Your account"}</h2>
    <div className={styles.factGrid}>{([
      { title: zh ? "周额度" : "Weekly quota", window: facts.weekly, notApplicable: false },
      { title: "5h", window: facts.short, notApplicable: facts.noShortWindow },
    ]).map((item) => <article key={item.title} className={styles.factCard}>
      <div className={styles.localHeading}><h3>{item.title}</h3><strong>{percentValueLabel(item.window?.remainingPercent)}</strong></div>
      <p>{item.notApplicable ? (zh ? "此套餐无独立 5h 限额" : "No separate 5h limit on this plan")
        : item.window ? `${zh ? "重置时间：" : "Resets: "}${publicEvidenceTime(item.window.resetsAt, language)}`
          : zh ? "尚未取得此窗口" : "This window is not available yet"}</p>
      <small>{item.notApplicable ? "—" : item.window ? resetCountdown(item.window.resetsAt, now, language)
        : "—"}</small>
    </article>)}<article className={styles.factCard}>
      <div className={styles.localHeading}><h3>{zh ? "可用重置卡" : "Available reset cards"}</h3><strong>{credits.count ?? "—"}</strong></div>
      <p>{credits.label}</p>
      <small>{credits.message}</small>
    </article></div>
    <p className={styles.note}>{failed
      ? (zh ? "更新失败，显示上次读数。" : "Update failed. Showing the last saved values.")
      : facts.observedAt ? `${facts.fresh ? (zh ? "最近观测：" : "Last observed: ") : (zh ? "已有快照：" : "Saved snapshot: ")}${publicEvidenceTime(facts.observedAt, language)}`
        : (zh ? "等待本机账号的有效额度快照。" : "Waiting for a valid local quota snapshot.")}</p>
    <details className={styles.sourceDisclosure}><summary>{zh ? "本机额度变化记录" : "Local quota changes"}</summary>
      {observations ?? <p className={styles.note}>{zh ? "正在读取额度变化…" : "Loading quota changes…"}</p>}
    </details>
  </section>;
}

/** Local updates stay in this panel; they do not remount the public timeline. */
export function LocalResetPanel({ language }: { language: Language }) {
  const [envelope, setEnvelope] = useState<DesktopStatusEnvelope | null>(getCapacityStatusSnapshot);
  const [failed, setFailed] = useState(false);
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (!isDesktopApp) return;
    let active = true;
    let pending = false;
    let receivedEvent = false;
    const apply = (next: DesktopStatusEnvelope) => {
      if (!active) return;
      setEnvelope((current) => current ? acceptNewerEnvelope(current, next) : next);
    };
    const unsubscribe = subscribeToCapacityStatus((next) => {
      receivedEvent = true;
      apply(next);
      if (active) setFailed(false);
    });
    void getCapacityCachedStatus().then((cached) => { if (cached) apply(cached); }).catch(() => undefined);
    const inspect = async () => {
      if (!active || pending || document.visibilityState === "hidden") return;
      pending = true;
      receivedEvent = false;
      try {
        apply(await getCapacityStatus());
        if (active) setFailed(false);
      } catch {
        if (active && !receivedEvent) setFailed(true);
      } finally { pending = false; }
    };
    const focus = () => { setNow(Date.now()); void inspect(); };
    const visibility = () => { if (document.visibilityState === "visible") focus(); };
    void inspect();
    // No extra quota-fetch loop: the shared service already refreshes quota.
    // This minute tick updates only the displayed countdown while visible.
    const tick = window.setInterval(() => {
      if (document.visibilityState === "visible") setNow(Date.now());
    }, 60_000);
    window.addEventListener("focus", focus);
    document.addEventListener("visibilitychange", visibility);
    return () => {
      active = false;
      unsubscribe();
      window.clearInterval(tick);
      window.removeEventListener("focus", focus);
      document.removeEventListener("visibilitychange", visibility);
    };
  }, []);
  return <LocalResetFacts envelope={envelope} language={language} now={now} failed={failed}
    observations={<LocalQuotaChanges envelope={envelope} language={language} />} />;
}
