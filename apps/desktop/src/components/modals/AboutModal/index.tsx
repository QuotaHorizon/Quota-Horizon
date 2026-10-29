import { useState } from "react";
import {
  Copy,
  MessageSquareText,
  RotateCcw,
  Server,
  ShieldCheck,
  X,
} from "lucide-react";
import type { Language, Translate } from "../../../i18n";
import type { AppBuildInfo } from "../../../types";
import { buildIdentity, buildInfoText } from "./buildInfo";
import styles from "./index.module.less";

interface AboutModalProps {
  logoUrl: string;
  onClose: () => void;
  onFeedback: () => void;
  onCheckUpdate: () => void;
  checkUpdateLabel: string;
  version: string;
  build?: AppBuildInfo;
  language: Language;
  checkingUpdate?: boolean;
  t: Translate;
}

export function AboutModal({
  logoUrl,
  onClose,
  onFeedback,
  onCheckUpdate,
  checkUpdateLabel,
  version,
  build,
  language,
  checkingUpdate = false,
  t,
}: AboutModalProps) {
  const zh = language === "zh";
  const identity = buildIdentity(build, language);
  const [copyState, setCopyState] = useState<"idle" | "copied" | "failed">("idle");
  const copyBuild = async () => {
    try { await navigator.clipboard.writeText(buildInfoText(version, build)); setCopyState("copied"); }
    catch { setCopyState("failed"); }
  };
  const principles = [
    {
      icon: <ShieldCheck size={18} />,
      title: t("about.local.title"),
      description: t("about.local.description"),
    },
    {
      icon: <RotateCcw size={18} />,
      title: t("about.workflow.title"),
      description: t("about.workflow.description"),
    },
    {
      icon: <Server size={18} />,
      title: t("about.ecosystem.title"),
      description: t("about.ecosystem.description"),
    },
  ];

  return (
    <div className={`${styles.styleScope} modal-backdrop`} onClick={onClose}>
      <section
        className="modal about-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="about-modal-title"
        onClick={(event) => event.stopPropagation()}
      >
        <button type="button" className="modal-close" aria-label={t("about.close")} onClick={onClose}>
          <X size={19} />
        </button>

        <header className="about-hero">
          <div className="about-brand-mark">
            <img src={logoUrl} alt="" />
          </div>
          <div className="about-brand-copy">
            <span>{t("about.eyebrow")}</span>
            <h2 id="about-modal-title">QuotaHorizon</h2>
            <p>{t("about.tagline")}</p>
          </div>
          <div className="about-badges" aria-label={t("about.badges")}>
            <span>{t("about.badge.local")}</span>
            <span>{t("about.badge.desktop")}</span>
            <span>Apache-2.0</span>
          </div>
        </header>

        <div className="about-body">
          <p className="about-introduction">{t("about.description")}</p>

          <div className="about-principles">
            {principles.map((principle) => (
              <article key={principle.title}>
                <div>{principle.icon}</div>
                <span>
                  <b>{principle.title}</b>
                  <small>{principle.description}</small>
                </span>
              </article>
            ))}
          </div>

          <div className="about-version-panel">
            <div>
              <span>{t("about.currentVersion")}</span>
              <b>v{version}</b>
              <small>{identity.source}</small>
              {identity.platform && <small>{identity.platform}</small>}
              <small>{zh ? "构建 " : "Built "}{identity.builtAt ?? (zh ? "时间未记录" : "time unavailable")}</small>
            </div>
            <button type="button" className="about-copy-build" onClick={() => void copyBuild()}>
              <Copy size={14} />{copyState === "copied" ? (zh ? "已复制" : "Copied") : (zh ? "复制版本信息" : "Copy version info")}
            </button>
          </div>
          {copyState === "failed" && <p className="about-copy-status" role="status">{zh ? "复制失败，请重试" : "Could not copy. Please try again."}</p>}

          <div className="about-actions">
            <button type="button" onClick={onCheckUpdate} disabled={checkingUpdate} aria-busy={checkingUpdate}><RotateCcw size={15} className={checkingUpdate ? "spin" : undefined} />{checkingUpdate ? (zh ? "正在检查…" : "Checking…") : checkUpdateLabel}</button>
            <button type="button" onClick={onFeedback}>
              <MessageSquareText size={15} />
              {t("help.feedback")}
            </button>
          </div>

          <footer className="about-legal">
            <span>{t("about.license")}</span>
            <p>{t("about.disclaimer")}</p>
          </footer>
        </div>
      </section>
    </div>
  );
}
