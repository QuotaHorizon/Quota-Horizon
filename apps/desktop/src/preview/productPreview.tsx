// Standalone documentation fixture. Not imported by the native app entry point.
import React from "react";
import { createRoot } from "react-dom/client";
import { ConfigProvider } from "antd";
import { CapacityPopover } from "../components/CapacityPopover";
import { AccountsPage } from "../pages/AccountsPage";
import { LiveSourcesView } from "../pages/ResetIntelligencePage/LiveSourcesPanel";
import { ResetNoticeChip } from "../components/CapacityPopover/ResetNoticeChip";
import type { PublicResetTimeline } from "../pages/ResetIntelligencePage/types";
import type { Account } from "../types";
import { LANGUAGE_STORAGE_KEY, translate } from "../i18n";
import "antd/dist/reset.css";
import "../styles.css";
import "./productPreview.css";

const query = new URLSearchParams(location.search);
const scene = query.get("scene") ?? "accounts";
const language = query.get("lang") === "en" ? "en" : "zh";
const zh = language === "zh";
document.documentElement.lang = zh ? "zh-CN" : "en";
document.documentElement.dataset.theme = query.get("theme") === "dark" ? "dark" : "light";
// This origin is the standalone browser preview, never the native app store.
localStorage.setItem(LANGUAGE_STORAGE_KEY, language);
const now = Date.now();
const observed = new Date(now - 2 * 60_000).toISOString();
const noop = () => undefined;
const unchanged = async () => false;
const noIds = async () => [];
const accounts: Account[] = [
  { plan: "Pro", remaining: 72.4, short: null, email: "research@example.com" },
  { plan: "Plus", remaining: 81, short: 94, email: "studio@example.com" },
  { plan: "Plus", remaining: 36.8, short: 62, email: "writing@example.com" },
].map((sample, index) => ({
  id: `synthetic-account-${index}`, email: sample.email, note: "", expiresAt: "",
  privateDetails: { password: "", phoneNumber: "", totpSecret: "" },
  plan: sample.plan, active: index === 0, autoSwitchEnabled: false,
  autoSwitchPriority: index, autoSwitchThreshold: 0, localProxyCompatible: true,
  directSwitchCompatible: true, agentIdentity: false, official: true, metadataEditable: true,
  usage: {
    primary: sample.short == null ? null : { usedPercent: 100 - sample.short, remainingPercent: sample.short,
      windowMinutes: 300, resetsAt: now / 1000 + (index + 1) * 3600 },
    secondary: { usedPercent: 100 - sample.remaining, remainingPercent: sample.remaining,
      windowMinutes: 10080, resetsAt: now / 1000 + (index + 2) * 86400 },
    fetchedAt: new Date().toISOString(),
  },
}));
const timeline: PublicResetTimeline = {
  sources: [], entries: [], revisionCount: 0, evidenceFamilyCount: 0,
  insights: {
    forecast: { attemptedAt: observed, issue: null, value: {
      sourceUrl: "https://codex-reset.com/api/forecast", updatedAt: observed, checkedAt: observed,
      probability24h: 24, probability48h: 41, confidence: "low", mode: "model",
      lastResetAt: null, signalScore: null, signalUrl: null, signalPublishedAt: null,
      signalDeadline: null, signalCorrected: false,
    } },
    posts: { fetchedAt: observed, checkedAt: observed, posts: [{
      id: "synthetic-post", url: "https://example.com/synthetic-reset-discussion",
      text: "Synthetic preview: a discussion about reset cards. This sample demonstrates how the original text and reply context appear.",
      translatedText: "合成正文：这里展示重置卡相关讨论，以及原文和回复上下文的阅读方式。",
      publishedAt: new Date(now - 20 * 60_000).toISOString(), kind: "context", isReply: true,
      parent: { id: "synthetic-parent", author: "example",
        url: "https://example.com/synthetic-parent-discussion",
        text: "Synthetic question: where can I find the reset-card details?", checkedAt: observed },
    }] },
  },
};

const scenario = query.get("scenario");
if (scene === "radar" && scenario) {
  const sourceUrl = "https://quotaresets.com/api/v1/events.json";
  const postUrl = "https://x.com/thsottiaux/status/2105843926221660585";
  const target = new Date(now + (scenario === "elapsed" ? -1 : 4) * 3_600_000).toISOString();
  const published = new Date(now - 5 * 3_600_000).toISOString();
  const text = "Global reset landing tomorrow 10am PST for all paid ChatGPT accounts.";
  timeline.sources = [{ sourceId: "quotaresets", sourceUrl, lastAttemptAt: observed, lastSuccessAt: observed,
    issue: scenario === "stale" ? "request_failed" : null, acceptedRecords: 1, rejectedRecords: 0, skippedRecords: 0 }];
  timeline.entries = [{ disposition: scenario === "withdrawn" ? "retracted" : "needs_review", signal: {
    signalId: "synthetic-announcement", evidenceFamilyId: "synthetic-family", revision: 1, eventId: "synthetic-reset",
    kind: "global_full_reset", semantics: scenario === "withdrawn" ? "retracted" : "explicit_timed_reset", scope: "unknown",
    recordedAt: observed, occurredAt: null, title: "Synthetic reset announcement", summary: text,
    announcementTiming: { expectedAt: target, timeZone: "America/Los_Angeles", sourceUrl, cohort: "all paid ChatGPT accounts" },
    source: { canonicalUrl: postUrl, author: "@thsottiaux", review: "indirect", sourceClass: "official_social",
      publishedAt: published, collectedAt: observed, contentSha256: "a".repeat(64), parserVersion: "preview", discoveredVia: [sourceUrl] },
  } }];
  timeline.insights!.forecast.value!.probability24h = 17;
  timeline.insights!.forecast.value!.probability48h = 31;
  timeline.insights!.posts!.posts = [{ id: "2105843926221660585", url: postUrl, text,
    translatedText: "明天太平洋时间上午 10 点，所有付费 ChatGPT 账号将迎来全局重置。",
    publishedAt: published, kind: "notice", isReply: false, parent: null }];
}

function Preview() {
  if ("__TAURI_INTERNALS__" in window) return <p>This fixture is available only in a browser.</p>;
  const title = scene === "menu" ? (zh ? "菜单栏弹窗" : "Menu-bar popover")
    : scene === "radar" ? (zh ? "重置雷达" : "Reset radar") : (zh ? "多账号额度" : "Multiple-account quota");
  return <ConfigProvider theme={{ token: { colorPrimary: "#35ada7", borderRadius: 10 } }}>
    <main className={`product-preview product-preview--${scene}`}>
      <header className="preview-heading"><div><small>QuotaHorizon</small><h1>{title}</h1></div>
        <p>{zh ? "合成预览 · 账号、额度、概率、正文与时间均为样例" : "Synthetic preview · sample accounts, quota, probabilities, text and times"}</p></header>
      {scene === "menu" ? <div className="preview-popover"><CapacityPopover /></div>
        : scene === "radar" ? <><LiveSourcesView language={language} now={now}
          state={{ timeline, busy: false, failed: false }} onRefresh={noop} onOpenSource={noop} />
          {scenario && <div style={{ maxWidth: 350, marginTop: 20 }}><ResetNoticeChip timeline={timeline}
            insights={timeline.insights} now={now} language={language} onOpen={noop} /></div>}</>
        : <AccountsPage active accounts={accounts} providers={[]} loading={false} busyAccountId={null}
          refreshingAccountIds={[]} refreshingAll={false} refreshProgress={null} localProxy={null} proxyBusy={false}
          resetCredits={{}} onAdd={noop} onSwitch={noop} onDeactivate={noop} onRollbackLastChange={noop}
          rollbackLastChangeBusy={false} onCopyAuthJson={noop} onRefresh={noop} onRefreshAll={noop} onDelete={noop}
          onConsumeQuotaMany={noIds} onDeleteMany={noIds} onEnableMany={noIds} onDisableMany={noIds}
          onAutoSwitchEnabledChange={noop} autoSwitchBusyAccountId={null}
          onAutoSwitchPriorityChange={unchanged} autoSwitchPriorityBusyAccountId={null}
          onAutoSwitchThresholdChange={unchanged} autoSwitchThresholdBusyAccountId={null}
          onGlobalAutoSwitchThresholdChange={unchanged} onSaveNote={unchanged}
          onLoadAccountDetails={async (id) => accounts.find((account) => account.id === id) ?? null}
          onLoadResetCredits={noop} onUseResetCredit={noop} resetCreditBusyAccountId={null}
          onOpenaiAuthAccountChange={noop} onConcurrentRoutingChange={noop}
          privacyMode={false} privacyModeLoading={false} onPrivacyModeChange={noop}
          hideAccountNotes showUsageNetworkErrors={false} displayMode="cards" tokenUsageRefreshSeconds={0}
          language={language} t={(key, values) => translate(language, key, values)} />}
    </main>
  </ConfigProvider>;
}

const root = createRoot(document.getElementById("root")!);
root.render(<Preview />);
if (import.meta.hot) import.meta.hot.dispose(() => root.unmount());
