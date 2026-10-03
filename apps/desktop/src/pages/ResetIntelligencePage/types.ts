export type PublicResetKind = "global_full_reset" | "global_banked_reset_grant"
  | "quota_policy_change" | "service_incident" | "unclassified";

export type PublicEvidenceDisposition = "confirmed" | "announced" | "possible" | "negative"
  | "context" | "needs_review" | "corrected" | "retracted";

export interface PublicResetCopy { en: string; zh: string }

export interface PublicResetArchiveEntry {
  disposition: PublicEvidenceDisposition;
  title: PublicResetCopy;
  summary: PublicResetCopy;
  interpretation: PublicResetCopy;
  signal: {
    signalId: string;
    revision: number;
    evidenceFamilyId: string;
    eventId: string | null;
    kind: PublicResetKind;
    semantics?: "context_only" | "possible_signal" | "explicit_future_reset" | "explicit_timed_reset"
      | "confirmed_reset" | "negative_signal" | "corrected" | "retracted";
    scope: "broad_codex" | "personal" | "unknown";
    recordedAt: string;
    occurredAt: string | null;
    announcementTiming?: {
      expectedOn?: string | null; expectedAt?: string | null; timeZone: string; sourceUrl: string; cohort?: string | null;
    } | null;
    source: {
      canonicalUrl: string;
      author: string;
      review: "primary_reviewed" | "indirect" | "unreviewed";
      sourceClass: "official_announcement" | "official_documentation" | "official_social"
        | "official_status" | "independent_tracker" | "provider_ui_observation";
      publishedAt: string | null;
      collectedAt: string;
      contentSha256: string;
      parserVersion: string;
      discoveredVia: string[];
    };
  };
}

export interface PublicResetArchive {
  schemaVersion: 1;
  mode: "bundled_archive";
  reviewedAt: string;
  entries: PublicResetArchiveEntry[];
  evidenceFamilyCount: number;
  historyComplete: false;
  forecastStatus: "abstained";
}

export type PublicSourceId = "quotaresets" | "codex_reset" | "codex_reset_posts" | "openai_status";
export type PublicSourceIssue = "request_failed" | "http_error" | "invalid_response" | "schema_changed" | "response_too_large" | "upstream_stale";

export interface PublicSourceStatus {
  sourceId: PublicSourceId;
  sourceUrl: string;
  lastAttemptAt: string | null;
  lastSuccessAt: string | null;
  issue: PublicSourceIssue | null;
  acceptedRecords: number;
  rejectedRecords: number;
  skippedRecords: number;
}

export interface PublicTimelineEntry {
  signal: PublicResetArchiveEntry["signal"] & { title: string; summary: string };
  disposition: PublicEvidenceDisposition;
}

export interface PublicResetTimeline {
  sources: PublicSourceStatus[];
  entries: PublicTimelineEntry[];
  revisionCount: number;
  evidenceFamilyCount: number;
  insights?: PublicInsights;
  radar?: RadarView;
  changes?: { items: PublicRevisionChange[]; totalCount: number; readVersion?: number };
}

export interface PublicRevisionChange {
  previous: PublicTimelineEntry["signal"] | null;
  current: PublicTimelineEntry["signal"];
  unread?: boolean;
}

export interface PublicReadReceipt { signalId: string; revision: number }

export interface ExternalResetForecast {
  sourceUrl: string; updatedAt: string; checkedAt: string;
  probability24h: number; probability48h: number; confidence: string; mode: string;
  lastResetAt: string | null; signalScore: number | null; signalUrl: string | null;
  signalPublishedAt: string | null; signalDeadline: string | null; signalCorrected: boolean;
  signalState?: string | null;
}

export interface PublicParentPost {
  id: string; author: string; url: string; text: string | null; checkedAt: string | null;
}

export interface PublicPost {
  id: string; url: string; text: string; translatedText: string | null; publishedAt: string;
  kind: "grant" | "reset_report" | "notice" | "limits" | "context";
  isReply: boolean | null; parent: PublicParentPost | null;
}

export interface PublicInsights {
  forecast: { value: ExternalResetForecast | null; attemptedAt: string | null; issue: PublicSourceIssue | null };
  posts: { fetchedAt: string; checkedAt: string; posts: PublicPost[] } | null;
}

export interface RadarPoint { at: string; probability24h: number; probability48h: number }
export interface RadarForecast {
  id: string; name: string; url: string; method: "cadence" | "statements" | "mixed";
  probability24h: number | null; probability48h: number | null;
  baseline24h: number | null; baseline48h: number | null;
  updatedAt: string | null; collectedAt: string; lastResetAt: string | null;
  evidenceUrls: string[]; usesCommunity: boolean; issue: PublicSourceIssue | null;
  exclusion: "unavailable" | "fetch_failed" | "reset_mismatch" | "stale" | "community_overlap" | null;
  weight: number; history: RadarPoint[];
}
export interface RadarOpinion {
  id: string; channel: string; author: string; url: string; text: string; publishedAt: string;
  stance: "optimistic" | "uncertain" | "pessimistic" | "wish" | "observation";
  reason: string; evidenceUrls: string[]; horizonHours: number | null;
}
export interface RadarChannel {
  id: string; name: string; url: string; query: string; attemptedAt: string; successAt: string | null;
  issue: PublicSourceIssue | null; scanned: number; truncated: boolean;
}
export interface RadarView {
  updatedAt: string | null;
  forecasts: RadarForecast[];
  community: {
    channels: RadarChannel[]; opinions: RadarOpinion[];
    optimistic: number; uncertain: number; pessimistic: number; wishes: number; observations: number;
    authors: number; communities: number; independentAuthors: number; duplicates: number;
    optimisticShare: number | null; previousShare: number | null;
  };
  estimate: {
    probability24h: number; probability48h: number; pooled24h: number; pooled48h: number;
    communityAdjustment24h: number; communityAdjustment48h: number;
    sourceCount: number; spread24h: number; spread48h: number; evidenceGroups: number; modelVersion: string;
  } | null;
  history: RadarPoint[];
}
