import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { resetCalendarRecords } from "./resetCalendarModel";
import { upcomingResetNotices } from "./upcomingResetModel";
import { forecastPresentation, postReading } from "./resetBriefingModel";
import { ResetBriefing } from "./ResetBriefing";
import { ResetNoticeChip } from "../../components/CapacityPopover/ResetNoticeChip";
import type { PublicPost, PublicResetTimeline, PublicTimelineEntry } from "./types";

const now = Date.parse("2026-09-26T03:00:00Z");
function data(): PublicResetTimeline {
  const sourceUrl = "https://codex-reset.com/api/timeline";
  const entry: PublicTimelineEntry = { disposition: "needs_review", signal: {
    signalId: "grant-original", revision: 1, evidenceFamilyId: "same-original-post", eventId: "same-explicit-event",
    kind: "global_banked_reset_grant", scope: "unknown", semantics: "possible_signal", occurredAt: null,
    recordedAt: "2026-09-26T02:55:00Z", title: "Synthetic card rollout", summary: "We are loading a banked reset into all accounts of Plus and Pro users.",
    source: { canonicalUrl: "https://x.com/thsottiaux/status/123456", author: "Tibo", review: "indirect", sourceClass: "official_social",
      publishedAt: "2026-09-26T01:00:00Z", collectedAt: "2026-09-26T02:55:00Z", contentSha256: "a".repeat(64), parserVersion: "fixture",
      discoveredVia: [sourceUrl] },
  } };
  const post: PublicPost = { id: "123456", url: entry.signal.source.canonicalUrl, text: entry.signal.summary,
    translatedText: null, kind: "grant", isReply: false, parent: null, publishedAt: entry.signal.source.publishedAt! };
  return { entries: [entry], revisionCount: 1, evidenceFamilyCount: 1,
    sources: [{ sourceId: "codex_reset", sourceUrl, lastSuccessAt: "2026-09-26T02:55:00Z", lastAttemptAt: "2026-09-26T02:55:00Z",
      acceptedRecords: 1, rejectedRecords: 0, skippedRecords: 0, issue: null }],
    insights: { posts: { fetchedAt: "2026-09-26T02:55:00Z", checkedAt: "2026-09-26T02:55:00Z", posts: [post] },
      forecast: { attemptedAt: "2026-09-26T02:55:00Z", issue: null, value: {
        sourceUrl: "https://codex-reset.com/api/forecast", updatedAt: "2026-09-26T02:55:00Z", checkedAt: "2026-09-26T02:55:00Z",
        probability24h: 20, probability48h: 35, confidence: "low", mode: "model", lastResetAt: null,
        signalScore: 93, signalUrl: post.url, signalPublishedAt: post.publishedAt, signalDeadline: "2026-09-26T07:00:00Z", signalCorrected: false,
      } } },
  };
}
function correction(value: PublicResetTimeline, semantics: PublicTimelineEntry["signal"]["semantics"]) {
  const mirror = structuredClone(value.entries[0]); mirror.signal.signalId = "other-mirror"; mirror.signal.semantics = semantics;
  // The indirect domain disposition is not guaranteed to say "negative".
  mirror.disposition = "needs_review";
  mirror.signal.source.discoveredVia = ["https://quotaresets.com/api/v1/events.json"];
  value.entries.push(mirror); return mirror;
}

describe("public corrections across visible surfaces", () => {
  it("does not leave a grant on the heatmap when another mirror denies it", () => {
    const value = data();
    expect(resetCalendarRecords([], value.entries, "zh", now)[0].time).not.toBeNull();
    correction(value, "negative_signal");
    expect(upcomingResetNotices(value, now)[0].state).toBe("withdrawn");
    expect(resetCalendarRecords([], value.entries, "zh", now)[0]).toMatchObject({ withdrawn: true, time: null, reviewed: false });
  });

  it("marks a collected post corrected when its semantic state disagrees with a generic disposition", () => {
    for (const semantics of ["negative_signal", "corrected", "retracted"] as const) {
      const value = data(); correction(value, semantics);
      const result = postReading(value.insights!.posts!.posts[0], "zh", value);
      expect(result.title).toContain("更正"); expect(result.tone).toBe("notice");
    }
  });

  it("stops watching a corrected statement without rewriting the site's probabilities", () => {
    const value = data(); correction(value, "corrected");
    const html = renderToStaticMarkup(<ResetBriefing timeline={value} language="zh" now={now} />);
    expect(html).toContain("关联线索已更正");
    expect(html).not.toContain("· 关注中");
    expect(html).toContain("20%"); expect(html).toContain("35%"); expect(html).toContain("93/100");
    expect(value.insights!.forecast.value!.signalCorrected).toBe(false);
    expect(forecastPresentation(value.insights, now, false, value)).toMatchObject({ signalActive: false, signalWithdrawn: true, stale: false });
    const chip = renderToStaticMarkup(<ResetNoticeChip timeline={value} insights={value.insights} now={now} language="zh" onOpen={() => undefined} />);
    expect(chip).toContain("预告有更正"); expect(chip).toContain("第三方 24h 20%");
    expect(chip).toContain('data-state="withdrawn"');
  });

  it("follows explicit event identity to a separate correction without guessing from nearby dates", () => {
    const value = data(); const negative = correction(value, "negative_signal");
    negative.signal.evidenceFamilyId = "separate-correction-post";
    negative.signal.source.canonicalUrl = "https://x.com/thsottiaux/status/654321";
    expect(upcomingResetNotices(value, now).every((notice) => notice.state === "withdrawn")).toBe(true);
    expect(resetCalendarRecords([], value.entries, "zh", now)[0]).toMatchObject({ withdrawn: true, time: null });
    expect(postReading(value.insights!.posts!.posts[0], "zh", value).tone).toBe("notice");
    negative.signal.eventId = null;
    expect(postReading(value.insights!.posts!.posts[0], "zh", value).tone).toBe("grant");
    expect(resetCalendarRecords([], value.entries, "zh", now)[0].withdrawn).toBe(false);
  });

  it("uses the latest revision when a tracker explicitly reinstates the claim", () => {
    const value = data(); const withdrawn = correction(value, "retracted");
    withdrawn.disposition = "retracted";
    const restored = structuredClone(withdrawn); restored.signal.revision = 2; restored.signal.semantics = "possible_signal";
    restored.disposition = "needs_review"; value.entries.push(restored);
    expect(upcomingResetNotices(value, now)[0].state).toBe("announced_delivery");
    expect(resetCalendarRecords([], value.entries, "zh", now)[0].withdrawn).toBe(false);
    expect(postReading(value.insights!.posts!.posts[0], "zh", value).tone).toBe("grant");
  });

  it("does not borrow a denial from an unrelated post, personal observation or different known kind", () => {
    for (const variant of ["unrelated", "personal", "provider-ui", "other-kind"] as const) {
      const value = data(); const negative = correction(value, "negative_signal");
      if (variant === "unrelated") {
        negative.signal.evidenceFamilyId = "unrelated"; negative.signal.eventId = "another-event";
        negative.signal.source.canonicalUrl = "https://x.com/thsottiaux/status/999999";
      }
      if (variant === "personal") negative.signal.scope = "personal";
      if (variant === "provider-ui") negative.signal.source.sourceClass = "provider_ui_observation";
      if (variant === "other-kind") {
        negative.signal.kind = "global_full_reset";
        negative.signal.source.canonicalUrl = "https://x.com/thsottiaux/status/999999";
      }
      expect(resetCalendarRecords([], value.entries, "zh", now)[0].withdrawn).toBe(false);
      expect(postReading(value.insights!.posts!.posts[0], "zh", value).tone).toBe("grant");
      expect(forecastPresentation(value.insights, now).value?.probability24h).toBe(20);
      expect(forecastPresentation(value.insights, now, false, value).signalWithdrawn).toBe(false);
    }
  });
});
